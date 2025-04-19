use alloy_primitives::{BlockHash, BlockNumber};
use futures_util::StreamExt;
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW},
    table::Value,
    tables,
    transaction::{DbTx, DbTxMut},
    RawKey, RawTable, RawValue,
};
use reth_era::{era1_file::Era1Reader, execution_types::DecodeCompressed};
use reth_era_downloader::{EraStream, HttpClient};
use reth_primitives_traits::{Block, FullBlockBody, FullBlockHeader, NodePrimitives};
use reth_provider::{
    BlockWriter, ProviderError, StaticFileProviderFactory, StaticFileSegment, StaticFileWriter,
};
use reth_storage_api::{DBProvider, HeaderProvider, NodePrimitivesProvider, StorageLocation};
use std::{fs::File, sync::mpsc};

/// Imports blocks from `downloader` using `provider`.
///
/// Returns current block height.
pub fn import<H, P, B, BB, BH>(
    mut downloader: EraStream<H>,
    provider: &P,
) -> eyre::Result<BlockNumber>
where
    B: Block<Header = BH, Body = BB>,
    BH: FullBlockHeader + Value,
    BB: FullBlockBody<
        Transaction = <<P as NodePrimitivesProvider>::Primitives as NodePrimitives>::SignedTx,
        OmmerHeader = BH,
    >,
    H: HttpClient + Clone + Send + Sync + 'static + Unpin,
    P: DBProvider<Tx: DbTxMut> + StaticFileProviderFactory + BlockWriter<Block = B>,
    <P as NodePrimitivesProvider>::Primitives: NodePrimitives<BlockHeader = BH, BlockBody = BB>,
{
    let (tx, rx) = mpsc::channel();

    // Handle IO-bound async download in a background tokio task
    tokio::spawn(async move {
        while let Some(file) = downloader.next().await {
            tx.send(Some(file))?;
        }
        tx.send(None)
    });

    let static_file_provider = provider.static_file_provider();

    // Consistency check of expected headers in static files vs DB is done on provider::sync_gap
    // when poll_execute_ready is polled.
    let mut last_header_number = static_file_provider
        .get_highest_static_file_block(StaticFileSegment::Headers)
        .unwrap_or_default();

    // Find the latest total difficulty
    let mut td = static_file_provider
        .header_td_by_number(last_header_number)?
        .ok_or(ProviderError::TotalDifficultyNotFound(last_header_number))?;

    // Although headers were downloaded in reverse order, the collector iterates it in ascending
    // order
    let mut writer = static_file_provider.latest_writer(StaticFileSegment::Headers)?;

    // Database cursor for hash to number index
    let mut cursor_header_numbers =
        provider.tx_ref().cursor_write::<RawTable<tables::HeaderNumbers>>()?;
    let mut first_sync = false;

    // If we only have the genesis block hash, then we are at first sync, and we can remove it,
    // add it to the collector and use tx.append on all hashes.
    if provider.tx_ref().entries::<RawTable<tables::HeaderNumbers>>()? == 1 {
        if let Some((hash, block_number)) = cursor_header_numbers.last()? {
            if block_number.value()? == 0 {
                cursor_header_numbers.delete_current()?;
                first_sync = true;
                cursor_header_numbers.append(hash, &block_number)?;
            }
        }
    }

    while let Some(file) = rx.recv()? {
        let file = File::open(file?)?;
        let mut reader = Era1Reader::new(file);

        for block in reader.iter() {
            let block = block?;
            let header: BH = block.header.decode()?;
            let body: BB = block.body.decode()?;
            let number = header.number();

            if number == 0 {
                continue;
            }

            let hash = header.hash_slow();
            last_header_number = number;

            // Increase total difficulty
            td += header.difficulty();

            // Append to Headers segment
            writer.append_header(&header, td, &hash)?;

            // Write bodies to database.
            provider.append_block_bodies(
                vec![(header.number(), Some(body))],
                // We are writing transactions directly to static files.
                StorageLocation::StaticFiles,
            )?;

            // Write block hash to block number index entry
            let key = RawKey::<BlockHash>::from(hash);
            let value = RawValue::<BlockNumber>::from(number);

            if first_sync {
                cursor_header_numbers.append(key, &value)?;
            } else {
                cursor_header_numbers.upsert(key, &value)?;
            }
        }
    }

    Ok(last_header_number)
}

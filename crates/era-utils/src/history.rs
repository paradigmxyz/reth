use alloy_primitives::{BlockHash, BlockNumber, U256};
use futures_util::{Stream, StreamExt};
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW},
    table::Value,
    tables,
    transaction::{DbTx, DbTxMut},
    RawKey, RawTable, RawValue,
};
use reth_era::{
    e2s_types::E2sError,
    era1_file::{BlockTupleIterator, Era1Reader},
    execution_types::{BlockTuple, DecodeCompressed},
};
use reth_era_downloader::EraMeta;
use reth_etl::Collector;
use reth_fs_util as fs;
use reth_primitives_traits::{Block, FullBlockBody, FullBlockHeader, NodePrimitives};
use reth_provider::{
    providers::StaticFileProviderRWRefMut, writer::UnifiedStorageWriter, BlockWriter,
    ProviderError, StaticFileProviderFactory, StaticFileSegment, StaticFileWriter,
};
use reth_stages_types::{
    CheckpointBlockRange, EntitiesCheckpoint, HeadersCheckpoint, StageCheckpoint, StageId,
};
use reth_storage_api::{
    errors::ProviderResult, DBProvider, DatabaseProviderFactory, HeaderProvider,
    NodePrimitivesProvider, StageCheckpointWriter, StorageLocation,
};
use std::{
    collections::Bound,
    error::Error,
    fmt::{Display, Formatter},
    io::{Read, Seek},
    iter::Map,
    ops::RangeBounds,
    sync::mpsc,
};
use tracing::info;

/// Imports blocks from `downloader` using `provider`.
///
/// Returns current block height.
pub fn import<Downloader, Era, PF, B, BB, BH>(
    mut downloader: Downloader,
    provider_factory: &PF,
    hash_collector: &mut Collector<BlockHash, BlockNumber>,
) -> eyre::Result<BlockNumber>
where
    B: Block<Header = BH, Body = BB>,
    BH: FullBlockHeader + Value,
    BB: FullBlockBody<
        Transaction = <<<PF as DatabaseProviderFactory>::ProviderRW as NodePrimitivesProvider>::Primitives as NodePrimitives>::SignedTx,
        OmmerHeader = BH,
    >,
    Downloader: Stream<Item = eyre::Result<Era>> + Send + 'static + Unpin,
    Era: EraMeta + Send + 'static,
    PF: DatabaseProviderFactory<
        ProviderRW: BlockWriter<Block = B>
            + DBProvider
            + StaticFileProviderFactory<Primitives: NodePrimitives<Block = B, BlockHeader = BH, BlockBody = BB>>
            + StageCheckpointWriter,
    > + StaticFileProviderFactory<Primitives = <<PF as DatabaseProviderFactory>::ProviderRW as NodePrimitivesProvider>::Primitives>,
{
    let (tx, rx) = mpsc::channel();

    // Handle IO-bound async download in a background tokio task
    tokio::spawn(async move {
        while let Some(file) = downloader.next().await {
            tx.send(Some(file))?;
        }
        tx.send(None)
    });

    let static_file_provider = provider_factory.static_file_provider();

    // Consistency check of expected headers in static files vs DB is done on provider::sync_gap
    // when poll_execute_ready is polled.
    let mut height = static_file_provider
        .get_highest_static_file_block(StaticFileSegment::Headers)
        .unwrap_or_default();

    // Find the latest total difficulty
    let mut td = static_file_provider
        .header_td_by_number(height)?
        .ok_or(ProviderError::TotalDifficultyNotFound(height))?;

    while let Some(meta) = rx.recv()? {
        let from = height;
        let provider = provider_factory.database_provider_rw()?;

        height = process(
            &meta?,
            &mut static_file_provider.latest_writer(StaticFileSegment::Headers)?,
            &provider,
            hash_collector,
            &mut td,
            height..,
        )?;

        save_stage_checkpoints(&provider, from, height, height, height)?;

        UnifiedStorageWriter::commit(provider)?;
    }

    let provider = provider_factory.database_provider_rw()?;

    build_index(&provider, hash_collector)?;

    UnifiedStorageWriter::commit(provider)?;

    Ok(height)
}

/// Saves progress of ERA import into stages sync.
///
/// Since the ERA import does the same work as `HeaderStage` and `BodyStage`, it needs to inform
/// these stages that this work has already been done. Otherwise, there might be some conflict with
/// database integrity.
pub fn save_stage_checkpoints<P>(
    provider: &P,
    from: BlockNumber,
    to: BlockNumber,
    processed: u64,
    total: u64,
) -> ProviderResult<()>
where
    P: StageCheckpointWriter,
{
    provider.save_stage_checkpoint(
        StageId::Headers,
        StageCheckpoint::new(to).with_headers_stage_checkpoint(HeadersCheckpoint {
            block_range: CheckpointBlockRange { from, to },
            progress: EntitiesCheckpoint { processed, total },
        }),
    )?;
    provider.save_stage_checkpoint(
        StageId::Bodies,
        StageCheckpoint::new(to)
            .with_entities_stage_checkpoint(EntitiesCheckpoint { processed, total }),
    )?;
    Ok(())
}

/// Extracts block headers and bodies from `meta` and appends them using `writer` and `provider`.
///
/// Adds on to `total_difficulty` and collects hash to height using `hash_collector`.
///
/// Skips all blocks below the [`start_bound`] of `block_numbers` and stops when reaching past the
/// [`end_bound`] or the end of the file.
///
/// Returns last block height.
///
/// [`start_bound`]: RangeBounds::start_bound
/// [`end_bound`]: RangeBounds::end_bound
pub fn process<Era, P, B, BB, BH>(
    meta: &Era,
    writer: &mut StaticFileProviderRWRefMut<'_, <P as NodePrimitivesProvider>::Primitives>,
    provider: &P,
    hash_collector: &mut Collector<BlockHash, BlockNumber>,
    total_difficulty: &mut U256,
    block_numbers: impl RangeBounds<BlockNumber>,
) -> eyre::Result<BlockNumber>
where
    B: Block<Header = BH, Body = BB>,
    BH: FullBlockHeader + Value,
    BB: FullBlockBody<
        Transaction = <<P as NodePrimitivesProvider>::Primitives as NodePrimitives>::SignedTx,
        OmmerHeader = BH,
    >,
    Era: EraMeta + ?Sized,
    P: DBProvider<Tx: DbTxMut> + NodePrimitivesProvider + BlockWriter<Block = B>,
    <P as NodePrimitivesProvider>::Primitives: NodePrimitives<BlockHeader = BH, BlockBody = BB>,
{
    let reader = open(meta)?;
    let iter =
        reader
            .iter()
            .map(Box::new(decode)
                as Box<dyn Fn(Result<BlockTuple, E2sError>) -> eyre::Result<(BH, BB)>>);
    let iter = ProcessIter { iter, era: meta };

    process_iter(iter, writer, provider, hash_collector, total_difficulty, block_numbers)
}

type ProcessInnerIter<R, BH, BB> =
    Map<BlockTupleIterator<R>, Box<dyn Fn(Result<BlockTuple, E2sError>) -> eyre::Result<(BH, BB)>>>;

/// An iterator that wraps era file extraction. After the final item [`EraMeta::mark_as_processed`]
/// is called to ensure proper cleanup.
#[derive(Debug)]
pub struct ProcessIter<'a, Era: ?Sized, R: Read, BH, BB>
where
    BH: FullBlockHeader + Value,
    BB: FullBlockBody<OmmerHeader = BH>,
{
    iter: ProcessInnerIter<R, BH, BB>,
    era: &'a Era,
}

impl<'a, Era: EraMeta + ?Sized, R: Read, BH, BB> Display for ProcessIter<'a, Era, R, BH, BB>
where
    BH: FullBlockHeader + Value,
    BB: FullBlockBody<OmmerHeader = BH>,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.era.path().to_string_lossy(), f)
    }
}

impl<'a, Era, R, BH, BB> Iterator for ProcessIter<'a, Era, R, BH, BB>
where
    R: Read + Seek,
    Era: EraMeta + ?Sized,
    BH: FullBlockHeader + Value,
    BB: FullBlockBody<OmmerHeader = BH>,
{
    type Item = eyre::Result<(BH, BB)>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.iter.next() {
            Some(item) => Some(item),
            None => match self.era.mark_as_processed() {
                Ok(..) => None,
                Err(e) => Some(Err(e)),
            },
        }
    }
}

/// Opens the era file described by `meta`.
pub fn open<Era>(meta: &Era) -> eyre::Result<Era1Reader<std::fs::File>>
where
    Era: EraMeta + ?Sized,
{
    let file = fs::open(meta.path())?;
    let reader = Era1Reader::new(file);

    Ok(reader)
}

/// Extracts a pair of [`FullBlockHeader`] and [`FullBlockBody`] from [`BlockTuple`].
pub fn decode<BH, BB, E>(block: Result<BlockTuple, E>) -> eyre::Result<(BH, BB)>
where
    BH: FullBlockHeader + Value,
    BB: FullBlockBody<OmmerHeader = BH>,
    E: From<E2sError> + Error + Send + Sync + 'static,
{
    let block = block?;
    let header: BH = block.header.decode()?;
    let body: BB = block.body.decode()?;

    Ok((header, body))
}

/// Extracts block headers and bodies from `iter` and appends them using `writer` and `provider`.
///
/// Adds on to `total_difficulty` and collects hash to height using `hash_collector`.
///
/// Skips all blocks below the [`start_bound`] of `block_numbers` and stops when reaching past the
/// [`end_bound`] or the end of the file.
///
/// Returns last block height.
///
/// [`start_bound`]: RangeBounds::start_bound
/// [`end_bound`]: RangeBounds::end_bound
pub fn process_iter<P, B, BB, BH>(
    mut iter: impl Iterator<Item = eyre::Result<(BH, BB)>>,
    writer: &mut StaticFileProviderRWRefMut<'_, <P as NodePrimitivesProvider>::Primitives>,
    provider: &P,
    hash_collector: &mut Collector<BlockHash, BlockNumber>,
    total_difficulty: &mut U256,
    block_numbers: impl RangeBounds<BlockNumber>,
) -> eyre::Result<BlockNumber>
where
    B: Block<Header = BH, Body = BB>,
    BH: FullBlockHeader + Value,
    BB: FullBlockBody<
        Transaction = <<P as NodePrimitivesProvider>::Primitives as NodePrimitives>::SignedTx,
        OmmerHeader = BH,
    >,
    P: DBProvider<Tx: DbTxMut> + NodePrimitivesProvider + BlockWriter<Block = B>,
    <P as NodePrimitivesProvider>::Primitives: NodePrimitives<BlockHeader = BH, BlockBody = BB>,
{
    let mut last_header_number = match block_numbers.start_bound() {
        Bound::Included(&number) => number,
        Bound::Excluded(&number) => number.saturating_sub(1),
        Bound::Unbounded => 0,
    };
    let target = match block_numbers.end_bound() {
        Bound::Included(&number) => Some(number),
        Bound::Excluded(&number) => Some(number.saturating_add(1)),
        Bound::Unbounded => None,
    };

    for block in &mut iter {
        let (header, body) = block?;
        let number = header.number();

        if number <= last_header_number {
            continue;
        }
        if let Some(target) = target {
            if number > target {
                break;
            }
        }

        let hash = header.hash_slow();
        last_header_number = number;

        // Increase total difficulty
        *total_difficulty += header.difficulty();

        // Append to Headers segment
        writer.append_header(&header, *total_difficulty, &hash)?;

        // Write bodies to database.
        provider.append_block_bodies(
            vec![(header.number(), Some(body))],
            // We are writing transactions directly to static files.
            StorageLocation::StaticFiles,
        )?;

        hash_collector.insert(hash, number)?;
    }

    Ok(last_header_number)
}

/// Dumps the contents of `hash_collector` into [`tables::HeaderNumbers`].
pub fn build_index<P, B, BB, BH>(
    provider: &P,
    hash_collector: &mut Collector<BlockHash, BlockNumber>,
) -> eyre::Result<()>
where
    B: Block<Header = BH, Body = BB>,
    BH: FullBlockHeader + Value,
    BB: FullBlockBody<
        Transaction = <<P as NodePrimitivesProvider>::Primitives as NodePrimitives>::SignedTx,
        OmmerHeader = BH,
    >,
    P: DBProvider<Tx: DbTxMut> + NodePrimitivesProvider + BlockWriter<Block = B>,
    <P as NodePrimitivesProvider>::Primitives: NodePrimitives<BlockHeader = BH, BlockBody = BB>,
{
    let total_headers = hash_collector.len();
    info!(target: "era::history::import", total = total_headers, "Writing headers hash index");

    // Database cursor for hash to number index
    let mut cursor_header_numbers =
        provider.tx_ref().cursor_write::<RawTable<tables::HeaderNumbers>>()?;
    let mut first_sync = false;

    // If we only have the genesis block hash, then we are at first sync, and we can remove it,
    // add it to the collector and use tx.append on all hashes.
    if provider.tx_ref().entries::<RawTable<tables::HeaderNumbers>>()? == 1 {
        if let Some((hash, block_number)) = cursor_header_numbers.last()? {
            if block_number.value()? == 0 {
                hash_collector.insert(hash.key()?, 0)?;
                cursor_header_numbers.delete_current()?;
                first_sync = true;
            }
        }
    }

    let interval = (total_headers / 10).max(8192);

    // Build block hash to block number index
    for (index, hash_to_number) in hash_collector.iter()?.enumerate() {
        let (hash, number) = hash_to_number?;

        if index != 0 && index % interval == 0 {
            info!(target: "era::history::import", progress = %format!("{:.2}%", (index as f64 / total_headers as f64) * 100.0), "Writing headers hash index");
        }

        let hash = RawKey::<BlockHash>::from_vec(hash);
        let number = RawValue::<BlockNumber>::from_vec(number);

        if first_sync {
            cursor_header_numbers.append(hash, &number)?;
        } else {
            cursor_header_numbers.upsert(hash, &number)?;
        }
    }

    Ok(())
}

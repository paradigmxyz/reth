use alloy_consensus::{BlockHeader, ReceiptWithBloom, RlpDecodableReceipt};
use alloy_primitives::{BlockHash, BlockNumber, U256};
use futures_util::{Stream, StreamExt};
use reth_codecs::Compact;
use reth_db_api::{
    cursor::{DbCursorRO, DbCursorRW},
    table::Value,
    tables,
    transaction::{DbTx, DbTxMut},
    RawKey, RawTable, RawValue,
};
use reth_era::{
    common::{decode::DecodeCompressedRlp, file_ops::StreamReader},
    e2s::error::E2sError,
    era::{file::EraReader, types::consensus::CompressedSignedBeaconBlock},
    era1::{file::Era1Reader, types::execution::BlockTuple},
    ere::{
        file::EreReader,
        types::execution::{try_receipt_from_slim, BlockTuple as EreBlockTuple},
    },
};
use reth_era_downloader::EraMeta;
use reth_etl::Collector;
use reth_fs_util as fs;
use reth_primitives_traits::{Block, BlockBody, FullBlockBody, FullBlockHeader, NodePrimitives};
use reth_provider::{
    providers::StaticFileProviderRWRefMut, BlockReader, BlockWriter, StaticFileProviderFactory,
    StaticFileSegment, StaticFileWriter,
};
use reth_stages_types::{
    CheckpointBlockRange, EntitiesCheckpoint, HeadersCheckpoint, StageCheckpoint, StageId,
};
use reth_storage_api::{
    errors::{ProviderError, ProviderResult},
    BlockBodyIndicesProvider, DBProvider, DatabaseProviderFactory, NodePrimitivesProvider,
    StageCheckpointWriter,
};
use std::{collections::Bound, error::Error, ops::RangeBounds, sync::mpsc};
use tracing::info;

/// A decoded ERA block: header, body, and optionally its receipts.
type EraBlock<BH, BB, R> = (BH, BB, Option<Vec<R>>);

/// The receipt type of the node primitives behind provider `P`.
type ReceiptOf<P> = <<P as NodePrimitivesProvider>::Primitives as NodePrimitives>::Receipt;

/// Reads execution `(header, body, receipts)` tuples out of an ERA file.
///
/// `receipts` is `None` when `decode_receipts` is `false`, or the file has none (`.era` never
/// does; `.ere` receipts are optional). `decode_receipts = false` skips decoding entirely.
///
/// Per-format seam of the import pipeline.
pub trait EraBlockReader<BH, BB, R> {
    /// Opens the ERA file at `meta` and iterates its execution blocks.
    fn blocks<M: EraMeta + ?Sized>(
        meta: &M,
        decode_receipts: bool,
    ) -> eyre::Result<impl Iterator<Item = eyre::Result<EraBlock<BH, BB, R>>>>;
}

/// [`EraBlockReader`] for `.era1` files.
#[derive(Debug)]
pub struct Era1;

impl<BH, BB, R> EraBlockReader<BH, BB, R> for Era1
where
    BH: FullBlockHeader + Value,
    BB: FullBlockBody<OmmerHeader = BH>,
    R: RlpDecodableReceipt,
{
    fn blocks<M: EraMeta + ?Sized>(
        meta: &M,
        decode_receipts: bool,
    ) -> eyre::Result<impl Iterator<Item = eyre::Result<EraBlock<BH, BB, R>>>> {
        let reader: Era1Reader<std::fs::File> = open(meta)?;
        Ok(reader
            .iter()
            .map(move |block| decode_with_receipts::<BH, BB, R, E2sError>(block, decode_receipts)))
    }
}

impl<BH, BB, R> EraBlockReader<BH, BB, R> for Ere
where
    BH: FullBlockHeader + Value,
    BB: FullBlockBody<OmmerHeader = BH>,
    R: 'static,
{
    fn blocks<M: EraMeta + ?Sized>(
        meta: &M,
        decode_receipts: bool,
    ) -> eyre::Result<impl Iterator<Item = eyre::Result<EraBlock<BH, BB, R>>>> {
        let reader: EreReader<std::fs::File> = open(meta)?;
        Ok(reader.iter().map(move |block| Self::decode_with_receipts(block, decode_receipts)))
    }
}

/// [`EraBlockReader`] for `.ere`/`.erae` files.
#[derive(Debug)]
pub struct Ere;

impl Ere {
    /// Extracts a `(header, body)` pair from an ERE block tuple, whose header and body are
    /// RLP-compressed. Ignores any receipts entry; callers that need receipts should use
    /// [`decode_with_receipts`](Self::decode_with_receipts).
    pub fn decode<BH, BB, E>(block: Result<EreBlockTuple, E>) -> eyre::Result<(BH, BB)>
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

    /// Like [`decode`](Self::decode), but also extracts receipts when `decode_receipts` is `true`.
    /// `receipts` is `None` if `decode_receipts` is `false`, or if the block tuple carries no
    /// receipts entry (`.ere` receipts are optional per spec).
    pub fn decode_with_receipts<BH, BB, R, E>(
        block: Result<EreBlockTuple, E>,
        decode_receipts: bool,
    ) -> eyre::Result<EraBlock<BH, BB, R>>
    where
        BH: FullBlockHeader + Value,
        BB: FullBlockBody<OmmerHeader = BH>,
        R: 'static,
        E: From<E2sError> + Error + Send + Sync + 'static,
    {
        let block = block?;
        let header: BH = block.header.decode()?;
        let body: BB = block.body.decode()?;
        let receipts = decode_receipts
            .then(|| block.receipts.as_ref().map(|r| r.decode_receipts()))
            .flatten()
            .transpose()?
            .map(|slim| {
                slim.into_iter()
                    .map(|receipt| {
                        try_receipt_from_slim(receipt).ok_or_else(|| {
                            eyre::eyre!(
                                "`.ere` receipts import is not supported for this node's \
                                 receipt type"
                            )
                        })
                    })
                    .collect::<eyre::Result<Vec<R>>>()
            })
            .transpose()?;

        Ok((header, body, receipts))
    }
}

/// [`EraBlockReader`] for consensus-layer `.era` files.
///
/// `.era` files store consensus `SignedBeaconBlock`s. Post-merge
/// blocks embed an execution payload; this source SSZ-decodes each beacon block, extracts that
/// payload, and converts it into an execution `(header, body)` pair. Pre-merge slots carry no
/// payload and are skipped. `.era` files carry no receipt data at all, so this source always
/// yields `None` for receipts.
#[derive(Debug)]
pub struct Era;

impl<BH, BB, R> EraBlockReader<BH, BB, R> for Era
where
    BH: FullBlockHeader,
    BB: FullBlockBody,
{
    /// `.era` files carry no receipt data, so `decode_receipts` has no effect.
    fn blocks<M: EraMeta + ?Sized>(
        meta: &M,
        _decode_receipts: bool,
    ) -> eyre::Result<impl Iterator<Item = eyre::Result<EraBlock<BH, BB, R>>>> {
        let reader: EraReader<std::fs::File> = open(meta)?;
        let mut buf = Vec::new();
        Ok(reader.iter().filter_map(move |block| {
            Self::decode(block, &mut buf)
                .map(|opt| opt.map(|(header, body)| (header, body, None)))
                .transpose()
        }))
    }
}

impl Era {
    /// Decodes the execution `(header, body)` embedded in a consensus `SignedBeaconBlock`.
    ///
    /// Returns `Ok(None)` for pre-merge slots, which carry no execution payload.
    pub fn decode<BH, BB>(
        block: Result<CompressedSignedBeaconBlock, E2sError>,
        buf: &mut Vec<u8>,
    ) -> eyre::Result<Option<(BH, BB)>>
    where
        BH: FullBlockHeader,
        BB: FullBlockBody,
    {
        let Some(alloy_consensus::Block { header, body }) =
            block?.decode_execution_block::<<BB as BlockBody>::Transaction>()?
        else {
            return Ok(None);
        };
        // The beacon payload decodes into alloy execution types; re-encode and decode into the
        // node's own primitives through the same RLP representation the `.era1`/`.ere` paths use.
        Ok(Some((reencode_rlp(&header, buf)?, reencode_rlp(&body, buf)?)))
    }
}

/// Re-encodes an alloy execution type as RLP and decodes it back into the node's primitive type.
///
/// `.era` files yield alloy execution headers/bodies, while the import pipeline writes the node's
/// own primitives. Both share the canonical execution RLP encoding, so a round-trip bridges them
/// without requiring a direct `From` conversion.
fn reencode_rlp<T, U>(value: &T, buf: &mut Vec<u8>) -> eyre::Result<U>
where
    T: alloy_rlp::Encodable,
    U: alloy_rlp::Decodable,
{
    buf.clear();
    alloy_rlp::Encodable::encode(value, buf);
    Ok(<U as alloy_rlp::Decodable>::decode(&mut buf.as_slice())?)
}

/// Opens the ERA file at `meta` with the format's [`StreamReader`].
pub fn open<Reader>(meta: &(impl EraMeta + ?Sized)) -> eyre::Result<Reader>
where
    Reader: StreamReader<std::fs::File>,
{
    Ok(Reader::new(fs::open(meta.path())?))
}

/// Imports blocks from `downloader`, decoding each file with the [`EraBlockReader`] `S`.
///
/// When `to_block` is set, the import stops after reaching that block height; otherwise it
/// continues until the source has no more files.
///
/// When `store_receipts` is set, each block's receipts are also decoded and appended to the
/// `Receipts` static file segment; a source file that carries no receipts for a block (only
/// possible for `.ere` files, whose receipts are optional per spec) is then treated as an error.
///
/// Returns current block height.
pub fn import<S, Downloader, Era, PF, B, BB, BH>(
    mut downloader: Downloader,
    provider_factory: &PF,
    hash_collector: &mut Collector<BlockHash, BlockNumber>,
    to_block: Option<BlockNumber>,
    store_receipts: bool,
) -> eyre::Result<BlockNumber>
where
    S: EraBlockReader<BH, BB, ReceiptOf<<PF as DatabaseProviderFactory>::ProviderRW>>,
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
            + BlockBodyIndicesProvider
            + StaticFileProviderFactory<Primitives: NodePrimitives<Block = B, BlockHeader = BH, BlockBody = BB>>
            + StageCheckpointWriter,
    > + StaticFileProviderFactory<Primitives = <<PF as DatabaseProviderFactory>::ProviderRW as NodePrimitivesProvider>::Primitives>,
    ReceiptOf<<PF as DatabaseProviderFactory>::ProviderRW>: Compact,
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

    let end = to_block.map_or(Bound::Unbounded, Bound::Included);

    while let Some(meta) = rx.recv()? {
        let meta = meta?;
        let from = height;
        let provider = provider_factory.database_provider_rw()?;

        // Headers and receipts are separate static file segments, each with its own writer, only
        // open the receipts one when `--with-receipts` was requested.
        let mut receipts_writer = store_receipts
            .then(|| static_file_provider.latest_writer(StaticFileSegment::Receipts))
            .transpose()?;

        height = process::<S, _, _, _, _>(
            &meta,
            &mut static_file_provider.latest_writer(StaticFileSegment::Headers)?,
            receipts_writer.as_mut(),
            &provider,
            hash_collector,
            (Bound::Included(height), end),
        )?;

        // Drop the receipts writer's lock before `provider.commit()`, which locks every static
        // file segment (including receipts) via `has_unwind_queued`.
        drop(receipts_writer);

        save_stage_checkpoints(&provider, from, height, height, height)?;

        provider.commit()?;

        info!(target: "era::history::import", first = from, last = height, file = %meta.path().display(), "Imported ERA file");

        if to_block.is_some_and(|to| height >= to) {
            break;
        }
    }

    let provider = provider_factory.database_provider_rw()?;

    build_index(&provider, hash_collector)?;

    provider.commit()?;

    Ok(height)
}

/// Saves progress of ERA import into stages sync.
///
/// Only marks `Headers`/`Bodies` done, never `Execution`: unlike this import, execution also
/// produces state, which imported receipts alone don't. `ExecutionStage::ensure_consistency`
/// prunes any imported receipts back before a real sync re-executes the range.
pub fn save_stage_checkpoints<P>(
    provider: P,
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

/// Reads `meta` with the [`EraBlockReader`] `S`, appends its blocks within `block_numbers`, and
/// marks `meta` processed if the file was fully consumed. Returns last block height.
pub fn process<S, P, B, BB, BH>(
    meta: &(impl EraMeta + ?Sized),
    writer: &mut StaticFileProviderRWRefMut<'_, <P as NodePrimitivesProvider>::Primitives>,
    receipts_writer: Option<
        &mut StaticFileProviderRWRefMut<'_, <P as NodePrimitivesProvider>::Primitives>,
    >,
    provider: &P,
    hash_collector: &mut Collector<BlockHash, BlockNumber>,
    block_numbers: impl RangeBounds<BlockNumber>,
) -> eyre::Result<BlockNumber>
where
    S: EraBlockReader<BH, BB, ReceiptOf<P>>,
    B: Block<Header = BH, Body = BB>,
    BH: FullBlockHeader + Value,
    BB: FullBlockBody<
        Transaction = <<P as NodePrimitivesProvider>::Primitives as NodePrimitives>::SignedTx,
        OmmerHeader = BH,
    >,
    P: DBProvider<Tx: DbTxMut>
        + NodePrimitivesProvider
        + BlockWriter<Block = B>
        + BlockBodyIndicesProvider,
    <P as NodePrimitivesProvider>::Primitives: NodePrimitives<BlockHeader = BH, BlockBody = BB>,
    ReceiptOf<P>: Compact,
{
    let decode_receipts = receipts_writer.is_some();
    let iter = S::blocks(meta, decode_receipts)?
        .map(Some)
        .chain(std::iter::once_with(|| match meta.mark_as_processed() {
            Ok(()) => None,
            Err(error) => Some(Err(error)),
        }))
        .flatten();

    process_iter(iter, writer, receipts_writer, provider, hash_collector, block_numbers)
}

/// Extracts a `(header, body)` pair from [`BlockTuple`].
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

/// Like [`decode`], but also extracts receipts when `decode_receipts` is `true`. `receipts` is
/// `Some` whenever `decode_receipts` is `true`: `era1`'s `CompressedReceipts` entry is mandatory
/// per spec, so (unlike `.ere`) it is never `None` just because the file omits it.
pub fn decode_with_receipts<BH, BB, R, E>(
    block: Result<BlockTuple, E>,
    decode_receipts: bool,
) -> eyre::Result<EraBlock<BH, BB, R>>
where
    BH: FullBlockHeader + Value,
    BB: FullBlockBody<OmmerHeader = BH>,
    R: RlpDecodableReceipt,
    E: From<E2sError> + Error + Send + Sync + 'static,
{
    let block = block?;
    let header: BH = block.header.decode()?;
    let body: BB = block.body.decode()?;
    let receipts = decode_receipts
        .then(|| -> eyre::Result<_> {
            let receipts: Vec<ReceiptWithBloom<R>> = block.receipts.decode()?;
            Ok(receipts.into_iter().map(|with_bloom| with_bloom.receipt).collect())
        })
        .transpose()?;

    Ok((header, body, receipts))
}

/// Extracts block headers, bodies and (optionally) receipts from `iter` and appends them using
/// `writer`, `receipts_writer` and `provider`.
///
/// Collects hash to height using `hash_collector`.
///
/// Skips all blocks below the [`start_bound`] of `block_numbers` and stops when reaching past the
/// [`end_bound`] or the end of the file.
///
/// When `receipts_writer` is `Some`, every block must carry receipts, a block without them,
/// possible for `.ere` files, whose receipts are optional per spec is an error.
///
/// Returns last block height.
///
/// [`start_bound`]: RangeBounds::start_bound
/// [`end_bound`]: RangeBounds::end_bound
pub fn process_iter<P, B, BB, BH>(
    mut iter: impl Iterator<Item = eyre::Result<EraBlock<BH, BB, ReceiptOf<P>>>>,
    writer: &mut StaticFileProviderRWRefMut<'_, <P as NodePrimitivesProvider>::Primitives>,
    mut receipts_writer: Option<
        &mut StaticFileProviderRWRefMut<'_, <P as NodePrimitivesProvider>::Primitives>,
    >,
    provider: &P,
    hash_collector: &mut Collector<BlockHash, BlockNumber>,
    block_numbers: impl RangeBounds<BlockNumber>,
) -> eyre::Result<BlockNumber>
where
    B: Block<Header = BH, Body = BB>,
    BH: FullBlockHeader + Value,
    BB: FullBlockBody<
        Transaction = <<P as NodePrimitivesProvider>::Primitives as NodePrimitives>::SignedTx,
        OmmerHeader = BH,
    >,
    P: DBProvider<Tx: DbTxMut>
        + NodePrimitivesProvider
        + BlockWriter<Block = B>
        + BlockBodyIndicesProvider,
    <P as NodePrimitivesProvider>::Primitives: NodePrimitives<BlockHeader = BH, BlockBody = BB>,
    ReceiptOf<P>: Compact,
{
    let mut last_header_number = match block_numbers.start_bound() {
        Bound::Included(&number) => number,
        Bound::Excluded(&number) => number.saturating_add(1),
        Bound::Unbounded => 0,
    };
    let target = match block_numbers.end_bound() {
        Bound::Included(&number) => Some(number),
        Bound::Excluded(&number) => Some(number.saturating_sub(1)),
        Bound::Unbounded => None,
    };

    for block in &mut iter {
        let (header, body, receipts) = block?;
        let number = header.number();

        if number <= last_header_number {
            continue;
        }
        if let Some(target) = target &&
            number > target
        {
            break;
        }

        // Reject gaps: the import marks the Headers/Bodies stages complete up to `height`, so a
        // non-contiguous append would leave earlier blocks missing while the stages report done.
        if number != last_header_number + 1 {
            eyre::bail!(
                "non-contiguous ERA import: expected block {}, got {number}; the execution \
                 database must be synced up to block {} before importing this file",
                last_header_number + 1,
                number - 1,
            );
        }

        let hash = header.hash_slow();
        last_header_number = number;

        // Append to Headers segment
        writer.append_header(&header, &hash)?;

        // Write bodies to database.
        provider.append_block_bodies(vec![(header.number(), Some(&body))])?;

        if let Some(receipts_writer) = receipts_writer.as_deref_mut() {
            provider.write_block_receipts(receipts_writer, number, receipts)?;
        }

        hash_collector.insert(hash, number)?;
    }

    Ok(last_header_number)
}

/// Lets a provider append one block's receipts to a `Receipts` static file writer, using itself
/// to look up the block's transaction range.
trait BlockReceiptsWriterExt: BlockBodyIndicesProvider {
    /// Appends `receipts` for block `number` to `receipts_writer`.
    ///
    /// Errors if the block has no receipts, or if the receipt count doesn't match the block's
    /// transaction count.
    fn write_block_receipts<N: NodePrimitives>(
        &self,
        receipts_writer: &mut StaticFileProviderRWRefMut<'_, N>,
        number: BlockNumber,
        receipts: Option<Vec<N::Receipt>>,
    ) -> eyre::Result<()>
    where
        N::Receipt: Compact;
}

impl<P: BlockBodyIndicesProvider> BlockReceiptsWriterExt for P {
    fn write_block_receipts<N: NodePrimitives>(
        &self,
        receipts_writer: &mut StaticFileProviderRWRefMut<'_, N>,
        number: BlockNumber,
        receipts: Option<Vec<N::Receipt>>,
    ) -> eyre::Result<()>
    where
        N::Receipt: Compact,
    {
        let Some(block_receipts) = receipts else {
            eyre::bail!(
                "block {number} has no receipts in the imported ERA file; drop --with-receipts, \
                 or import files that carry receipts for every block (`.era1` always includes \
                 them; `.ere` receipts are optional per spec)"
            );
        };

        let indices = self
            .block_body_indices(number)?
            .ok_or_else(|| eyre::eyre!("missing block body indices for block {number}"))?;

        if block_receipts.len() as u64 != indices.tx_count {
            eyre::bail!(
                "receipt count mismatch for block {number}: {} receipt(s) for {} transaction(s)",
                block_receipts.len(),
                indices.tx_count,
            );
        }

        receipts_writer.increment_block(number)?;
        receipts_writer.append_receipts(
            (indices.first_tx_num..).zip(block_receipts.iter()).map(Ok::<_, ProviderError>),
        )?;

        Ok(())
    }
}

/// Dumps the contents of `hash_collector` into [`tables::HeaderNumbers`].
pub fn build_index<P>(
    provider: &P,
    hash_collector: &mut Collector<BlockHash, BlockNumber>,
) -> eyre::Result<()>
where
    P: DBProvider<Tx: DbTxMut>,
{
    let total_headers = hash_collector.len();
    info!(target: "era::history::import", total = total_headers, "Writing headers hash index");

    // Database cursor for hash to number index
    let mut cursor_header_numbers =
        provider.tx_ref().cursor_write::<RawTable<tables::HeaderNumbers>>()?;
    // If we only have the genesis block hash, then we are at first sync, and we can remove it,
    // add it to the collector and use tx.append on all hashes.
    let first_sync = if provider.tx_ref().entries::<RawTable<tables::HeaderNumbers>>()? == 1 &&
        let Some((hash, block_number)) = cursor_header_numbers.last()? &&
        block_number.value()? == 0
    {
        hash_collector.insert(hash.key()?, 0)?;
        cursor_header_numbers.delete_current()?;
        true
    } else {
        false
    };

    let interval = (total_headers / 10).max(8192);

    // Build block hash to block number index
    for (index, hash_to_number) in hash_collector.iter()?.enumerate() {
        let (hash, number) = hash_to_number?;

        if index != 0 && index.is_multiple_of(interval) {
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

/// Calculates the total difficulty for a given block number by summing the difficulty
/// of all blocks from genesis to the given block.
///
/// Very expensive - iterates through all blocks in batches of 1000.
///
/// Returns an error if any block is missing.
pub fn calculate_td_by_number<P>(provider: &P, num: BlockNumber) -> eyre::Result<U256>
where
    P: BlockReader,
{
    let mut total_difficulty = U256::ZERO;
    let mut start = 0;

    while start <= num {
        let end = (start + 1000 - 1).min(num);

        total_difficulty +=
            provider.headers_range(start..=end)?.iter().map(|h| h.difficulty()).sum::<U256>();

        start = end + 1;
    }

    Ok(total_difficulty)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_consensus::Header;
    use reth_db_common::init::init_genesis;
    use reth_ethereum_primitives::{Block, BlockBody};
    use reth_provider::{
        test_utils::create_test_provider_factory, DatabaseProviderFactory,
        StaticFileProviderFactory, StaticFileSegment, StaticFileWriter,
    };
    use std::{cell::Cell, path::Path};
    use tempfile::tempdir;

    struct TestEra;

    impl<R> EraBlockReader<Header, BlockBody, R> for TestEra {
        fn blocks<M: EraMeta + ?Sized>(
            _meta: &M,
            _decode_receipts: bool,
        ) -> eyre::Result<impl Iterator<Item = eyre::Result<(Header, BlockBody, Option<Vec<R>>)>>>
        {
            Ok([1, 2].into_iter().map(|number| {
                Ok((Header { number, ..Default::default() }, BlockBody::default(), None))
            }))
        }
    }

    /// Like [`TestEra`], but yields `Some(vec![])` receipts for each (transaction-less) block.
    struct TestEraWithEmptyReceipts;

    impl<R> EraBlockReader<Header, BlockBody, R> for TestEraWithEmptyReceipts {
        fn blocks<M: EraMeta + ?Sized>(
            _meta: &M,
            _decode_receipts: bool,
        ) -> eyre::Result<impl Iterator<Item = eyre::Result<(Header, BlockBody, Option<Vec<R>>)>>>
        {
            Ok([1, 2].into_iter().map(|number| {
                Ok((Header { number, ..Default::default() }, BlockBody::default(), Some(vec![])))
            }))
        }
    }

    /// Like [`TestEra`], but yields a single receipt for a transaction-less block, provoking a
    /// receipt/transaction count mismatch.
    struct TestEraWithMismatchedReceipts;

    impl EraBlockReader<Header, BlockBody, reth_ethereum_primitives::Receipt>
        for TestEraWithMismatchedReceipts
    {
        fn blocks<M: EraMeta + ?Sized>(
            _meta: &M,
            _decode_receipts: bool,
        ) -> eyre::Result<
            impl Iterator<
                Item = eyre::Result<EraBlock<Header, BlockBody, reth_ethereum_primitives::Receipt>>,
            >,
        > {
            Ok(std::iter::once(Ok((
                Header { number: 1, ..Default::default() },
                BlockBody::default(),
                Some(vec![reth_ethereum_primitives::Receipt {
                    tx_type: alloy_consensus::TxType::Legacy,
                    success: true,
                    cumulative_gas_used: 0,
                    logs: vec![],
                }]),
            ))))
        }
    }

    #[derive(Debug)]
    struct TestMeta {
        marked: Cell<bool>,
    }

    impl EraMeta for TestMeta {
        fn mark_as_processed(&self) -> eyre::Result<()> {
            self.marked.set(true);
            Ok(())
        }

        fn path(&self) -> &Path {
            Path::new("test.era1")
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn import_stops_at_to_block() {
        let pf = create_test_provider_factory();
        init_genesis(&pf).unwrap();

        let folder = tempdir().unwrap();
        let mut hash_collector = Collector::new(4096, Some(folder.path().to_owned()));

        // Each file yields blocks 1 and 2; without `to_block` the import would reach 2.
        let stream = futures_util::stream::iter(vec![
            Ok(TestMeta { marked: Cell::new(false) }),
            Ok(TestMeta { marked: Cell::new(false) }),
        ]);

        let height = import::<TestEra, _, _, _, Block, _, _>(
            stream,
            &pf,
            &mut hash_collector,
            Some(1),
            false,
        )
        .unwrap();

        assert_eq!(height, 1);
    }

    #[test]
    fn process_does_not_mark_partially_consumed_file_processed() {
        let pf = create_test_provider_factory();
        init_genesis(&pf).unwrap();

        let static_file_provider = pf.static_file_provider();
        let mut writer = static_file_provider.latest_writer(StaticFileSegment::Headers).unwrap();
        let provider = pf.database_provider_rw().unwrap();
        let folder = tempdir().unwrap();
        let mut hash_collector = Collector::new(4096, Some(folder.path().to_owned()));
        let meta = TestMeta { marked: Cell::new(false) };

        let height = process::<TestEra, _, Block, _, _>(
            &meta,
            &mut writer,
            None,
            &provider,
            &mut hash_collector,
            0..=1,
        )
        .unwrap();

        assert_eq!(height, 1);
        assert!(!meta.marked.get());
    }

    #[test]
    fn process_iter_rejects_non_contiguous_blocks() {
        let pf = create_test_provider_factory();
        init_genesis(&pf).unwrap();

        let static_file_provider = pf.static_file_provider();
        let mut writer = static_file_provider.latest_writer(StaticFileSegment::Headers).unwrap();
        let provider = pf.database_provider_rw().unwrap();
        let folder = tempdir().unwrap();
        let mut hash_collector = Collector::new(4096, Some(folder.path().to_owned()));

        // Genesis DB sits at height 0, but the first block is 5: a gap that must be rejected
        // rather than appended (as a pre-merge `.era` import would otherwise produce).
        let blocks = [5u64, 6].into_iter().map(|number| {
            Ok((Header { number, ..Default::default() }, BlockBody::default(), None))
        });

        let result = process_iter::<_, Block, _, _>(
            blocks,
            &mut writer,
            None,
            &provider,
            &mut hash_collector,
            0..,
        );

        assert!(result.is_err());
    }

    #[test]
    fn process_writes_receipts_when_requested() {
        let pf = create_test_provider_factory();
        init_genesis(&pf).unwrap();

        let static_file_provider = pf.static_file_provider();
        let mut writer = static_file_provider.latest_writer(StaticFileSegment::Headers).unwrap();
        let mut receipts_writer =
            static_file_provider.latest_writer(StaticFileSegment::Receipts).unwrap();
        let provider = pf.database_provider_rw().unwrap();
        let folder = tempdir().unwrap();
        let mut hash_collector = Collector::new(4096, Some(folder.path().to_owned()));
        let meta = TestMeta { marked: Cell::new(false) };

        let height = process::<TestEraWithEmptyReceipts, _, Block, _, _>(
            &meta,
            &mut writer,
            Some(&mut receipts_writer),
            &provider,
            &mut hash_collector,
            0..=1,
        )
        .unwrap();
        receipts_writer.commit().unwrap();

        assert_eq!(height, 1);
        assert_eq!(
            static_file_provider.get_highest_static_file_block(StaticFileSegment::Receipts),
            Some(1)
        );
    }

    #[test]
    fn process_iter_errors_when_receipts_missing() {
        let pf = create_test_provider_factory();
        init_genesis(&pf).unwrap();

        let static_file_provider = pf.static_file_provider();
        let mut writer = static_file_provider.latest_writer(StaticFileSegment::Headers).unwrap();
        let mut receipts_writer =
            static_file_provider.latest_writer(StaticFileSegment::Receipts).unwrap();
        let provider = pf.database_provider_rw().unwrap();
        let folder = tempdir().unwrap();
        let mut hash_collector = Collector::new(4096, Some(folder.path().to_owned()));

        // `TestEra` never yields receipts; requesting them must fail rather than silently import
        // headers/bodies without them.
        let blocks: Vec<
            eyre::Result<EraBlock<Header, BlockBody, reth_ethereum_primitives::Receipt>>,
        > = vec![Ok((Header { number: 1, ..Default::default() }, BlockBody::default(), None))];

        let result = process_iter::<_, Block, _, _>(
            blocks.into_iter(),
            &mut writer,
            Some(&mut receipts_writer),
            &provider,
            &mut hash_collector,
            0..,
        );

        assert!(result.is_err());
    }

    #[test]
    fn process_errors_on_receipt_count_mismatch() {
        let pf = create_test_provider_factory();
        init_genesis(&pf).unwrap();

        let static_file_provider = pf.static_file_provider();
        let mut writer = static_file_provider.latest_writer(StaticFileSegment::Headers).unwrap();
        let mut receipts_writer =
            static_file_provider.latest_writer(StaticFileSegment::Receipts).unwrap();
        let provider = pf.database_provider_rw().unwrap();
        let folder = tempdir().unwrap();
        let mut hash_collector = Collector::new(4096, Some(folder.path().to_owned()));
        let meta = TestMeta { marked: Cell::new(false) };

        // One receipt for a transaction-less body: a count mismatch that must be rejected rather
        // than silently misaligning the Receipts static file against Transactions.
        let result = process::<TestEraWithMismatchedReceipts, _, Block, _, _>(
            &meta,
            &mut writer,
            Some(&mut receipts_writer),
            &provider,
            &mut hash_collector,
            0..,
        );

        assert!(result.is_err());
    }
}

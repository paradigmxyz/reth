use alloy_consensus::BlockHeader;
use alloy_eips::eip2718::Decodable2718;
use alloy_primitives::{BlockHash, BlockNumber, U256};
use alloy_rpc_types_beacon::block::{
    SignedBeaconBlockAltair, SignedBeaconBlockBellatrix, SignedBeaconBlockCapella,
    SignedBeaconBlockDeneb, SignedBeaconBlockElectra, SignedBeaconBlockPhase0,
};
use alloy_rpc_types_engine::{
    CancunPayloadFields, ExecutionPayload, ExecutionPayloadSidecar, ExecutionPayloadV1,
    ExecutionPayloadV2, ExecutionPayloadV3, PraguePayloadFields,
};
use futures_util::{Stream, StreamExt};
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
    ere::{file::EreReader, types::execution::BlockTuple as EreBlockTuple},
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
    errors::ProviderResult, DBProvider, DatabaseProviderFactory, NodePrimitivesProvider,
    StageCheckpointWriter,
};
use ssz::Decode;
use std::{collections::Bound, error::Error, ops::RangeBounds, sync::mpsc};
use tracing::info;

/// Reads execution `(header, body)` pairs out of an ERA file.
///
/// Per-format seam of the import pipeline.
pub trait EraBlockReader<BH, BB> {
    /// Opens the ERA file at `meta` and iterates its execution blocks.
    fn blocks<M: EraMeta + ?Sized>(
        meta: &M,
    ) -> eyre::Result<impl Iterator<Item = eyre::Result<(BH, BB)>>>;
}

/// [`EraBlockReader`] for `.era1` files.
#[derive(Debug)]
pub struct Era1;

impl<BH, BB> EraBlockReader<BH, BB> for Era1
where
    BH: FullBlockHeader + Value,
    BB: FullBlockBody<OmmerHeader = BH>,
{
    fn blocks<M: EraMeta + ?Sized>(
        meta: &M,
    ) -> eyre::Result<impl Iterator<Item = eyre::Result<(BH, BB)>>> {
        let reader: Era1Reader<std::fs::File> = open(meta)?;
        Ok(reader.iter().map(decode::<BH, BB, E2sError>))
    }
}

impl<BH, BB> EraBlockReader<BH, BB> for Ere
where
    BH: FullBlockHeader + Value,
    BB: FullBlockBody<OmmerHeader = BH>,
{
    fn blocks<M: EraMeta + ?Sized>(
        meta: &M,
    ) -> eyre::Result<impl Iterator<Item = eyre::Result<(BH, BB)>>> {
        let reader: EreReader<std::fs::File> = open(meta)?;
        Ok(reader.iter().map(Self::decode))
    }
}

/// [`EraBlockReader`] for `.ere`/`.erae` files.
#[derive(Debug)]
pub struct Ere;

impl Ere {
    /// Extracts a pair of [`FullBlockHeader`] and [`FullBlockBody`] from an ERE block tuple, whose
    /// header and body are RLP-compressed.
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
}

/// [`EraBlockReader`] for consensus-layer `.era` files.
///
/// `.era` files store consensus `SignedBeaconBlock`s. Post-merge
/// blocks embed an execution payload; this source SSZ-decodes each beacon block, extracts that
/// payload, and converts it into an execution `(header, body)` pair. Pre-merge slots carry no
/// payload and are skipped.
#[derive(Debug)]
pub struct Era;

impl<BH, BB> EraBlockReader<BH, BB> for Era
where
    BH: FullBlockHeader,
    BB: FullBlockBody,
{
    fn blocks<M: EraMeta + ?Sized>(
        meta: &M,
    ) -> eyre::Result<impl Iterator<Item = eyre::Result<(BH, BB)>>> {
        let reader: EraReader<std::fs::File> = open(meta)?;
        let mut buf = Vec::new();
        Ok(reader.iter().filter_map(move |block| Self::decode(block, &mut buf).transpose()))
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
        let ssz = block?.decompress()?;
        let Some(alloy_consensus::Block { header, body }) =
            decode_execution_block::<<BB as BlockBody>::Transaction>(&ssz)?
        else {
            return Ok(None);
        };
        // The beacon payload decodes into alloy execution types; re-encode and decode into the
        // node's own primitives through the same RLP representation the `.era1`/`.ere` paths use.
        Ok(Some((reencode_rlp(&header, buf)?, reencode_rlp(&body, buf)?)))
    }
}

/// SSZ-decodes a consensus `SignedBeaconBlock` and extracts its execution block, if it has one.
///
/// Only post-merge (Bellatrix and later) blocks carry a payload; the fork is found by
/// trial-decoding newest to oldest. Returns `Ok(None)` for genuine pre-merge slots, confirmed to
/// decode as a pre-merge block. Bytes matching no known fork are an error, so malformed data is
/// never silently dropped.
pub fn decode_execution_block<T: Decodable2718>(
    ssz: &[u8],
) -> eyre::Result<Option<alloy_consensus::Block<T>>> {
    if let Ok(beacon) = SignedBeaconBlockElectra::<ExecutionPayloadV3>::from_ssz_bytes(ssz) {
        let sidecar = ExecutionPayloadSidecar::v4(
            CancunPayloadFields {
                parent_beacon_block_root: beacon.message.parent_root,
                versioned_hashes: Vec::new(),
            },
            PraguePayloadFields::new(beacon.message.body.execution_requests.to_requests()),
        );
        let payload = ExecutionPayload::V3(beacon.message.body.execution_payload);
        return Ok(Some(payload.try_into_block_with_sidecar(&sidecar)?));
    }

    if let Ok(beacon) = SignedBeaconBlockDeneb::<ExecutionPayloadV3>::from_ssz_bytes(ssz) {
        let sidecar = ExecutionPayloadSidecar::v3(CancunPayloadFields {
            parent_beacon_block_root: beacon.message.parent_root,
            versioned_hashes: Vec::new(),
        });
        let payload = ExecutionPayload::V3(beacon.message.body.execution_payload);
        return Ok(Some(payload.try_into_block_with_sidecar(&sidecar)?));
    }

    if let Ok(beacon) = SignedBeaconBlockCapella::<ExecutionPayloadV2>::from_ssz_bytes(ssz) {
        let payload = ExecutionPayload::V2(beacon.message.body.execution_payload);
        return Ok(Some(payload.try_into_block()?));
    }

    if let Ok(beacon) = SignedBeaconBlockBellatrix::<ExecutionPayloadV1>::from_ssz_bytes(ssz) {
        let payload = ExecutionPayload::V1(beacon.message.body.execution_payload);
        return Ok(Some(payload.try_into_block()?));
    }

    // Pre-merge blocks carry no execution payload. Only skip a slot once it's confirmed to be a
    // valid pre-merge block; anything else is malformed data and must error rather than be skipped.
    if SignedBeaconBlockPhase0::from_ssz_bytes(ssz).is_ok() ||
        SignedBeaconBlockAltair::from_ssz_bytes(ssz).is_ok()
    {
        return Ok(None);
    }

    Err(eyre::eyre!(
        "consensus block ({} bytes) is not a valid SignedBeaconBlock of any known fork",
        ssz.len()
    ))
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
/// Returns current block height.
pub fn import<S, Downloader, Era, PF, B, BB, BH>(
    mut downloader: Downloader,
    provider_factory: &PF,
    hash_collector: &mut Collector<BlockHash, BlockNumber>,
    to_block: Option<BlockNumber>,
) -> eyre::Result<BlockNumber>
where
    S: EraBlockReader<BH, BB>,
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

    let end = to_block.map_or(Bound::Unbounded, Bound::Included);

    while let Some(meta) = rx.recv()? {
        let meta = meta?;
        let from = height;
        let provider = provider_factory.database_provider_rw()?;

        height = process::<S, _, _, _, _>(
            &meta,
            &mut static_file_provider.latest_writer(StaticFileSegment::Headers)?,
            &provider,
            hash_collector,
            (Bound::Included(height), end),
        )?;

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
/// Since the ERA import does the same work as `HeaderStage` and `BodyStage`, it needs to inform
/// these stages that this work has already been done. Otherwise, there might be some conflict with
/// database integrity.
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
    provider: &P,
    hash_collector: &mut Collector<BlockHash, BlockNumber>,
    block_numbers: impl RangeBounds<BlockNumber>,
) -> eyre::Result<BlockNumber>
where
    S: EraBlockReader<BH, BB>,
    B: Block<Header = BH, Body = BB>,
    BH: FullBlockHeader + Value,
    BB: FullBlockBody<
        Transaction = <<P as NodePrimitivesProvider>::Primitives as NodePrimitives>::SignedTx,
        OmmerHeader = BH,
    >,
    P: DBProvider<Tx: DbTxMut> + NodePrimitivesProvider + BlockWriter<Block = B>,
    <P as NodePrimitivesProvider>::Primitives: NodePrimitives<BlockHeader = BH, BlockBody = BB>,
{
    let iter = S::blocks(meta)?
        .map(Some)
        .chain(std::iter::once_with(|| match meta.mark_as_processed() {
            Ok(()) => None,
            Err(error) => Some(Err(error)),
        }))
        .flatten();

    process_iter(iter, writer, provider, hash_collector, block_numbers)
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
/// Collects hash to height using `hash_collector`.
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
        Bound::Excluded(&number) => number.saturating_add(1),
        Bound::Unbounded => 0,
    };
    let target = match block_numbers.end_bound() {
        Bound::Included(&number) => Some(number),
        Bound::Excluded(&number) => Some(number.saturating_sub(1)),
        Bound::Unbounded => None,
    };

    for block in &mut iter {
        let (header, body) = block?;
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

        hash_collector.insert(hash, number)?;
    }

    Ok(last_header_number)
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
    use alloy_primitives::B256;
    use alloy_rpc_types_beacon::{
        block::{BeaconBlock, BeaconBlockBodyPhase0, Eth1Data, SignedBeaconBlock},
        BlsSignature,
    };
    use reth_db_common::init::init_genesis;
    use reth_ethereum_primitives::{Block, BlockBody, TransactionSigned};
    use reth_provider::{
        test_utils::create_test_provider_factory, DatabaseProviderFactory,
        StaticFileProviderFactory, StaticFileSegment, StaticFileWriter,
    };
    use ssz::Encode;
    use std::{cell::Cell, path::Path};
    use tempfile::tempdir;

    struct TestEra;

    impl EraBlockReader<Header, BlockBody> for TestEra {
        fn blocks<M: EraMeta + ?Sized>(
            _meta: &M,
        ) -> eyre::Result<impl Iterator<Item = eyre::Result<(Header, BlockBody)>>> {
            Ok([1, 2]
                .into_iter()
                .map(|number| Ok((Header { number, ..Default::default() }, BlockBody::default()))))
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

    #[test]
    fn decode_execution_block_skips_genuine_pre_merge() {
        // A genuine pre-merge (phase0) beacon block carries no execution payload and yields `None`.
        let block = SignedBeaconBlock {
            message: BeaconBlock {
                slot: 0,
                proposer_index: 0,
                parent_root: B256::ZERO,
                state_root: B256::ZERO,
                body: BeaconBlockBodyPhase0 {
                    randao_reveal: BlsSignature::ZERO,
                    eth1_data: Eth1Data {
                        deposit_root: B256::ZERO,
                        deposit_count: 0,
                        block_hash: B256::ZERO,
                    },
                    graffiti: B256::ZERO,
                    proposer_slashings: vec![],
                    attester_slashings: vec![],
                    attestations: vec![],
                    deposits: vec![],
                    voluntary_exits: vec![],
                },
            },
            signature: BlsSignature::ZERO,
        };
        let ssz = block.as_ssz_bytes();
        assert!(decode_execution_block::<TransactionSigned>(&ssz).unwrap().is_none());
    }

    #[test]
    fn decode_execution_block_errors_on_malformed() {
        // Bytes that decode as no known fork are malformed, not pre-merge slots, and must error
        // instead of being silently dropped.
        assert!(decode_execution_block::<TransactionSigned>(&[]).is_err());
        assert!(decode_execution_block::<TransactionSigned>(&[0u8; 8]).is_err());
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

        let height =
            import::<TestEra, _, _, _, Block, _, _>(stream, &pf, &mut hash_collector, Some(1))
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
        let blocks = [5u64, 6]
            .into_iter()
            .map(|number| Ok((Header { number, ..Default::default() }, BlockBody::default())));

        let result = process_iter::<_, Block, _, _>(
            blocks,
            &mut writer,
            &provider,
            &mut hash_collector,
            0..,
        );

        assert!(result.is_err());
    }
}

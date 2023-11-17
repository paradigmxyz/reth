use super::{LoadedJar, SnapshotJarProvider};
use crate::{BlockHashReader, BlockNumReader, HeaderProvider, TransactionsProvider};
use dashmap::DashMap;
use parking_lot::RwLock;
use reth_db::{
    codecs::CompactU256,
    snapshot::{HeaderMask, TransactionMask},
};
use reth_interfaces::provider::{ProviderError, ProviderResult};
use reth_nippy_jar::NippyJar;
use reth_primitives::{
    snapshot::HighestSnapshots, Address, BlockHash, BlockHashOrNumber, BlockNumber, ChainInfo,
    Header, SealedHeader, SnapshotSegment, TransactionMeta, TransactionSigned,
    TransactionSignedNoHash, TxHash, TxNumber, B256, U256,
};
use revm::primitives::HashMap;
use std::{
    collections::BTreeMap,
    ops::{RangeBounds, RangeInclusive},
    path::{Path, PathBuf},
};
use tokio::sync::watch;

/// Alias type for a map that can be queried for transaction/block ranges from a block/transaction
/// segment respectively. It uses `BlockNumber` to represent the block end of a snapshot range or
/// `TxNumber` to represent the transaction end of a snapshot range.
///
/// Can be in one of the two formats:
/// - `HashMap<SnapshotSegment, BTreeMap<BlockNumber, RangeInclusive<TxNumber>>>`
/// - `HashMap<SnapshotSegment, BTreeMap<TxNumber, RangeInclusive<BlockNumber>>>`
type SegmentRanges = HashMap<SnapshotSegment, BTreeMap<u64, RangeInclusive<u64>>>;

/// [`SnapshotProvider`] manages all existing [`SnapshotJarProvider`].
#[derive(Debug, Default)]
pub struct SnapshotProvider {
    /// Maintains a map which allows for concurrent access to different `NippyJars`, over different
    /// segments and ranges.
    map: DashMap<(BlockNumber, SnapshotSegment), LoadedJar>,
    /// Available snapshot ranges on disk indexed by max blocks.
    snapshots_block_index: RwLock<SegmentRanges>,
    /// Available snapshot ranges on disk indexed by max transactions.
    snapshots_tx_index: RwLock<SegmentRanges>,
    /// Tracks the highest snapshot of every segment.
    highest_tracker: Option<watch::Receiver<Option<HighestSnapshots>>>,
    /// Directory where snapshots are located
    path: PathBuf,
}

impl SnapshotProvider {
    /// Creates a new [`SnapshotProvider`].
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            map: Default::default(),
            snapshots_block_index: Default::default(),
            snapshots_tx_index: Default::default(),
            highest_tracker: None,
            path: path.as_ref().to_path_buf(),
        }
    }

    /// Adds a highest snapshot tracker to the provider
    pub fn with_highest_tracker(
        mut self,
        highest_tracker: Option<watch::Receiver<Option<HighestSnapshots>>>,
    ) -> Self {
        self.highest_tracker = highest_tracker;
        self
    }

    /// Gets the [`SnapshotJarProvider`] of the requested segment and block.
    pub fn get_segment_provider_from_block(
        &self,
        segment: SnapshotSegment,
        block: BlockNumber,
        path: Option<&Path>,
    ) -> ProviderResult<SnapshotJarProvider<'_>> {
        self.get_segment_provider(
            segment,
            || self.get_segment_ranges_from_block(segment, block),
            path,
        )?
        .ok_or_else(|| ProviderError::MissingSnapshotBlock(segment, block))
    }

    /// Gets the [`SnapshotJarProvider`] of the requested segment and transaction.
    pub fn get_segment_provider_from_transaction(
        &self,
        segment: SnapshotSegment,
        tx: TxNumber,
        path: Option<&Path>,
    ) -> ProviderResult<SnapshotJarProvider<'_>> {
        self.get_segment_provider(
            segment,
            || self.get_segment_ranges_from_transaction(segment, tx),
            path,
        )?
        .ok_or_else(|| ProviderError::MissingSnapshotTx(segment, tx))
    }

    /// Gets the [`SnapshotJarProvider`] of the requested segment and block or transaction.
    pub fn get_segment_provider(
        &self,
        segment: SnapshotSegment,
        fn_ranges: impl Fn() -> Option<(RangeInclusive<BlockNumber>, RangeInclusive<TxNumber>)>,
        path: Option<&Path>,
    ) -> ProviderResult<Option<SnapshotJarProvider<'_>>> {
        // If we have a path, then get the block range and transaction range from its name.
        // Otherwise, check `self.available_snapshots`
        let snapshot_ranges = match path {
            Some(path) => {
                SnapshotSegment::parse_filename(path.file_name().ok_or_else(|| {
                    ProviderError::MissingSnapshotPath(segment, path.to_path_buf())
                })?)
                .and_then(|(parsed_segment, block_range, tx_range)| {
                    if parsed_segment == segment {
                        return Some((block_range, tx_range))
                    }
                    None
                })
            }
            None => fn_ranges(),
        };

        // Return cached `LoadedJar` or insert it for the first time, and then, return it.
        if let Some((block_range, tx_range)) = snapshot_ranges {
            return Ok(Some(self.get_or_create_jar_provider(segment, &block_range, &tx_range)?))
        }

        Ok(None)
    }

    /// Given a segment, block range and transaction range it returns a cached
    /// [`SnapshotJarProvider`]. TODO: we should check the size and pop N if there's too many.
    fn get_or_create_jar_provider(
        &self,
        segment: SnapshotSegment,
        block_range: &RangeInclusive<u64>,
        tx_range: &RangeInclusive<u64>,
    ) -> ProviderResult<SnapshotJarProvider<'_>> {
        let key = (*block_range.end(), segment);
        if let Some(jar) = self.map.get(&key) {
            Ok(jar.into())
        } else {
            self.map.insert(
                key,
                LoadedJar::new(NippyJar::load(
                    &self.path.join(segment.filename(block_range, tx_range)),
                )?)?,
            );
            Ok(self.map.get(&key).expect("qed").into())
        }
    }

    /// Gets a snapshot segment's block range and transaction range from the provider inner block
    /// index.
    fn get_segment_ranges_from_block(
        &self,
        segment: SnapshotSegment,
        block: u64,
    ) -> Option<(RangeInclusive<BlockNumber>, RangeInclusive<TxNumber>)> {
        let snapshots = self.snapshots_block_index.read();
        let segment_snapshots = snapshots.get(&segment)?;

        // It's more probable that the request comes from a newer block height, so we iterate
        // the snapshots in reverse.
        let mut snapshots_rev_iter = segment_snapshots.iter().rev().peekable();

        while let Some((block_end, tx_range)) = snapshots_rev_iter.next() {
            // `unwrap_or(0) is safe here as it sets block_start to 0 if the iterator is empty,
            // indicating the lowest height snapshot has been reached.
            let block_start =
                snapshots_rev_iter.peek().map(|(block_end, _)| *block_end + 1).unwrap_or(0);
            if block_start <= block {
                return Some((block_start..=*block_end, tx_range.clone()))
            }
        }
        None
    }

    /// Gets a snapshot segment's block range and transaction range from the provider inner
    /// transaction index.
    fn get_segment_ranges_from_transaction(
        &self,
        segment: SnapshotSegment,
        tx: u64,
    ) -> Option<(RangeInclusive<BlockNumber>, RangeInclusive<TxNumber>)> {
        let snapshots = self.snapshots_tx_index.read();
        let segment_snapshots = snapshots.get(&segment)?;

        // It's more probable that the request comes from a newer tx height, so we iterate
        // the snapshots in reverse.
        let mut snapshots_rev_iter = segment_snapshots.iter().rev().peekable();

        while let Some((tx_end, block_range)) = snapshots_rev_iter.next() {
            let tx_start = snapshots_rev_iter.peek().map(|(tx_end, _)| *tx_end + 1).unwrap_or(0);
            if tx_start <= tx {
                return Some((block_range.clone(), tx_start..=*tx_end))
            }
        }
        None
    }

    /// Gets the highest snapshot if it exists for a snapshot segment.
    pub fn get_highest_snapshot(&self, segment: SnapshotSegment) -> Option<BlockNumber> {
        self.highest_tracker
            .as_ref()
            .and_then(|tracker| tracker.borrow().and_then(|highest| highest.highest(segment)))
    }

    /// Iterates through segment snapshots in reverse order, executing a function until it returns
    /// some object. Useful for finding objects by [`TxHash`] or [`BlockHash`].
    pub fn find_snapshot<T>(
        &self,
        segment: SnapshotSegment,
        func: impl Fn(SnapshotJarProvider<'_>) -> ProviderResult<Option<T>>,
    ) -> ProviderResult<Option<T>> {
        let snapshots = self.snapshots_block_index.read();
        if let Some(segment_snapshots) = snapshots.get(&segment) {
            // It's more probable that the request comes from a newer block height, so we iterate
            // the snapshots in reverse.
            let mut snapshots_rev_iter = segment_snapshots.iter().rev().peekable();

            while let Some((block_end, tx_range)) = snapshots_rev_iter.next() {
                // `unwrap_or(0) is safe here as it sets block_start to 0 if the iterator
                // is empty, indicating the lowest height snapshot has been reached.
                let block_start =
                    snapshots_rev_iter.peek().map(|(block_end, _)| *block_end + 1).unwrap_or(0);

                if let Some(res) = func(self.get_or_create_jar_provider(
                    segment,
                    &(block_start..=*block_end),
                    tx_range,
                )?)? {
                    return Ok(Some(res))
                }
            }
        }

        Ok(None)
    }
}

impl HeaderProvider for SnapshotProvider {
    fn header(&self, block_hash: &BlockHash) -> ProviderResult<Option<Header>> {
        self.find_snapshot(SnapshotSegment::Headers, |jar_provider| {
            Ok(jar_provider
                .cursor()?
                .get_two::<HeaderMask<Header, BlockHash>>(block_hash.into())?
                .and_then(|(header, hash)| {
                    if &hash == block_hash {
                        return Some(header)
                    }
                    None
                }))
        })
    }

    fn header_by_number(&self, num: BlockNumber) -> ProviderResult<Option<Header>> {
        self.get_segment_provider_from_block(SnapshotSegment::Headers, num, None)?
            .header_by_number(num)
    }

    fn header_td(&self, block_hash: &BlockHash) -> ProviderResult<Option<U256>> {
        self.find_snapshot(SnapshotSegment::Headers, |jar_provider| {
            Ok(jar_provider
                .cursor()?
                .get_two::<HeaderMask<CompactU256, BlockHash>>(block_hash.into())?
                .and_then(|(td, hash)| (&hash == block_hash).then_some(td.0)))
        })
    }

    fn header_td_by_number(&self, num: BlockNumber) -> ProviderResult<Option<U256>> {
        self.get_segment_provider_from_block(SnapshotSegment::Headers, num, None)?
            .header_td_by_number(num)
    }

    fn headers_range(&self, _range: impl RangeBounds<BlockNumber>) -> ProviderResult<Vec<Header>> {
        todo!();
    }

    fn sealed_header(&self, num: BlockNumber) -> ProviderResult<Option<SealedHeader>> {
        self.get_segment_provider_from_block(SnapshotSegment::Headers, num, None)?
            .sealed_header(num)
    }

    fn sealed_headers_while(
        &self,
        _range: impl RangeBounds<BlockNumber>,
        _predicate: impl FnMut(&SealedHeader) -> bool,
    ) -> ProviderResult<Vec<SealedHeader>> {
        todo!()
    }
}

impl BlockHashReader for SnapshotProvider {
    fn block_hash(&self, num: u64) -> ProviderResult<Option<B256>> {
        self.get_segment_provider_from_block(SnapshotSegment::Headers, num, None)?.block_hash(num)
    }

    fn canonical_hashes_range(
        &self,
        _start: BlockNumber,
        _end: BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        todo!()
    }
}

impl BlockNumReader for SnapshotProvider {
    fn chain_info(&self) -> ProviderResult<ChainInfo> {
        todo!()
    }

    fn best_block_number(&self) -> ProviderResult<BlockNumber> {
        todo!()
    }

    fn last_block_number(&self) -> ProviderResult<BlockNumber> {
        todo!()
    }

    fn block_number(&self, _hash: B256) -> ProviderResult<Option<BlockNumber>> {
        todo!()
    }
}

impl TransactionsProvider for SnapshotProvider {
    fn transaction_id(&self, tx_hash: TxHash) -> ProviderResult<Option<TxNumber>> {
        self.find_snapshot(SnapshotSegment::Transactions, |jar_provider| {
            let mut cursor = jar_provider.cursor()?;
            if cursor
                .get_one::<TransactionMask<TransactionSignedNoHash>>((&tx_hash).into())?
                .and_then(|tx| (tx.hash() == tx_hash).then_some(tx))
                .is_some()
            {
                Ok(Some(cursor.number()))
            } else {
                Ok(None)
            }
        })
    }

    fn transaction_by_id(&self, num: TxNumber) -> ProviderResult<Option<TransactionSigned>> {
        self.get_segment_provider_from_transaction(SnapshotSegment::Transactions, num, None)?
            .transaction_by_id(num)
    }

    fn transaction_by_id_no_hash(
        &self,
        num: TxNumber,
    ) -> ProviderResult<Option<TransactionSignedNoHash>> {
        self.get_segment_provider_from_transaction(SnapshotSegment::Transactions, num, None)?
            .transaction_by_id_no_hash(num)
    }

    fn transaction_by_hash(&self, hash: TxHash) -> ProviderResult<Option<TransactionSigned>> {
        self.find_snapshot(SnapshotSegment::Transactions, |jar_provider| {
            Ok(jar_provider
                .cursor()?
                .get_one::<TransactionMask<TransactionSignedNoHash>>((&hash).into())?
                .map(|tx| tx.with_hash())
                .and_then(|tx| (tx.hash_ref() == &hash).then_some(tx)))
        })
    }

    fn transaction_by_hash_with_meta(
        &self,
        _hash: TxHash,
    ) -> ProviderResult<Option<(TransactionSigned, TransactionMeta)>> {
        todo!()
    }

    fn transaction_block(&self, _id: TxNumber) -> ProviderResult<Option<BlockNumber>> {
        todo!()
    }

    fn transactions_by_block(
        &self,
        _block_id: BlockHashOrNumber,
    ) -> ProviderResult<Option<Vec<TransactionSigned>>> {
        todo!()
    }

    fn transactions_by_block_range(
        &self,
        _range: impl RangeBounds<BlockNumber>,
    ) -> ProviderResult<Vec<Vec<TransactionSigned>>> {
        todo!()
    }

    fn senders_by_tx_range(
        &self,
        _range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Address>> {
        todo!()
    }

    fn transactions_by_tx_range(
        &self,
        _range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<reth_primitives::TransactionSignedNoHash>> {
        todo!()
    }

    fn transaction_sender(&self, id: TxNumber) -> ProviderResult<Option<Address>> {
        Ok(self.transaction_by_id_no_hash(id)?.and_then(|tx| tx.recover_signer()))
    }
}

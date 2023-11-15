use super::{LoadedJar, SnapshotJarProvider};
use crate::{BlockHashReader, BlockNumReader, HeaderProvider, TransactionsProvider};
use dashmap::DashMap;
use reth_interfaces::RethResult;
use reth_nippy_jar::NippyJar;
use reth_primitives::{
    snapshot::{HighestSnapshots, BLOCKS_PER_SNAPSHOT},
    Address, BlockHash, BlockHashOrNumber, BlockNumber, ChainInfo, Header, SealedHeader,
    SnapshotSegment, TransactionMeta, TransactionSigned, TransactionSignedNoHash, TxHash, TxNumber,
    B256, U256,
};
use std::{ops::RangeBounds, path::PathBuf};
use tokio::sync::watch;

/// SnapshotProvider
#[derive(Debug, Default)]
pub struct SnapshotProvider {
    /// Maintains a map which allows for concurrent access to different `NippyJars`, over different
    /// segments and ranges.
    map: DashMap<(BlockNumber, SnapshotSegment), LoadedJar>,
    /// Tracks the highest snapshot of every segment.
    highest_tracker: Option<watch::Receiver<Option<HighestSnapshots>>>,
    /// Directory where snapshots are located
    path: PathBuf,
}

impl SnapshotProvider {
    /// Creates a new [`SnapshotProvider`].
    pub fn new(path: PathBuf) -> Self {
        Self { map: Default::default(), highest_tracker: None, path }
    }

    /// Adds a highest snapshot tracker to the provider
    pub fn with_highest_tracker(
        mut self,
        highest_tracker: Option<watch::Receiver<Option<HighestSnapshots>>>,
    ) -> Self {
        self.highest_tracker = highest_tracker;
        self
    }

    /// Gets the provider of the requested segment and range.
    pub fn get_segment_provider(
        &self,
        segment: SnapshotSegment,
        block: BlockNumber,
        mut path: Option<PathBuf>,
    ) -> RethResult<SnapshotJarProvider<'_>> {
        // TODO this invalidates custom length snapshots.
        let snapshot = block / BLOCKS_PER_SNAPSHOT;
        let key = (snapshot, segment);

        if let Some(jar) = self.map.get(&key) {
            return Ok(jar.into())
        }

        if let Some(path) = &path {
            self.map.insert(key, LoadedJar::new(NippyJar::load(path)?)?);
        } else {
            path = Some(self.path.join(segment.filename(
                &((snapshot * BLOCKS_PER_SNAPSHOT)..=((snapshot + 1) * BLOCKS_PER_SNAPSHOT - 1)),
            )));
        }

        self.get_segment_provider(segment, block, path)
    }

    /// Gets the highest snapshot if it exists for a snapshot segment.
    pub fn get_highest_snapshot(&self, segment: SnapshotSegment) -> Option<BlockNumber> {
        self.highest_tracker
            .as_ref()
            .and_then(|tracker| tracker.borrow().and_then(|highest| highest.highest(segment)))
    }
}

impl HeaderProvider for SnapshotProvider {
    fn header(&self, _block_hash: &BlockHash) -> RethResult<Option<Header>> {
        todo!()
    }

    fn header_by_number(&self, num: BlockNumber) -> RethResult<Option<Header>> {
        self.get_segment_provider(SnapshotSegment::Headers, num, None)?.header_by_number(num)
    }

    fn header_td(&self, _block_hash: &BlockHash) -> RethResult<Option<U256>> {
        todo!()
    }

    fn header_td_by_number(&self, _number: BlockNumber) -> RethResult<Option<U256>> {
        todo!();
    }

    fn headers_range(&self, _range: impl RangeBounds<BlockNumber>) -> RethResult<Vec<Header>> {
        todo!();
    }

    fn sealed_headers_range(
        &self,
        _range: impl RangeBounds<BlockNumber>,
    ) -> RethResult<Vec<SealedHeader>> {
        todo!();
    }

    fn sealed_header(&self, _number: BlockNumber) -> RethResult<Option<SealedHeader>> {
        todo!();
    }
}

impl BlockHashReader for SnapshotProvider {
    fn block_hash(&self, _number: u64) -> RethResult<Option<B256>> {
        todo!()
    }

    fn canonical_hashes_range(
        &self,
        _start: BlockNumber,
        _end: BlockNumber,
    ) -> RethResult<Vec<B256>> {
        todo!()
    }
}

impl BlockNumReader for SnapshotProvider {
    fn chain_info(&self) -> RethResult<ChainInfo> {
        todo!()
    }

    fn best_block_number(&self) -> RethResult<BlockNumber> {
        todo!()
    }

    fn last_block_number(&self) -> RethResult<BlockNumber> {
        todo!()
    }

    fn block_number(&self, _hash: B256) -> RethResult<Option<BlockNumber>> {
        todo!()
    }
}

impl TransactionsProvider for SnapshotProvider {
    fn transaction_id(&self, _tx_hash: TxHash) -> RethResult<Option<TxNumber>> {
        todo!()
    }

    fn transaction_by_id(&self, num: TxNumber) -> RethResult<Option<TransactionSigned>> {
        // TODO `num` is provided after checking the index
        let block_num = num;
        self.get_segment_provider(SnapshotSegment::Transactions, block_num, None)?
            .transaction_by_id(num)
    }

    fn transaction_by_id_no_hash(
        &self,
        _id: TxNumber,
    ) -> RethResult<Option<TransactionSignedNoHash>> {
        todo!()
    }

    fn transaction_by_hash(&self, _hash: TxHash) -> RethResult<Option<TransactionSigned>> {
        todo!()
    }

    fn transaction_by_hash_with_meta(
        &self,
        _hash: TxHash,
    ) -> RethResult<Option<(TransactionSigned, TransactionMeta)>> {
        todo!()
    }

    fn transaction_block(&self, _id: TxNumber) -> RethResult<Option<BlockNumber>> {
        todo!()
    }

    fn transactions_by_block(
        &self,
        _block_id: BlockHashOrNumber,
    ) -> RethResult<Option<Vec<TransactionSigned>>> {
        todo!()
    }

    fn transactions_by_block_range(
        &self,
        _range: impl RangeBounds<BlockNumber>,
    ) -> RethResult<Vec<Vec<TransactionSigned>>> {
        todo!()
    }

    fn senders_by_tx_range(&self, _range: impl RangeBounds<TxNumber>) -> RethResult<Vec<Address>> {
        todo!()
    }

    fn transactions_by_tx_range(
        &self,
        _range: impl RangeBounds<TxNumber>,
    ) -> RethResult<Vec<reth_primitives::TransactionSignedNoHash>> {
        todo!()
    }

    fn transaction_sender(&self, _id: TxNumber) -> RethResult<Option<Address>> {
        todo!()
    }
}

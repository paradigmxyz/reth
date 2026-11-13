use super::{blocks::InMemoryBlocks, BlockState, ExecutedBlock, NewCanonicalChain};
use crate::{
    CanonStateNotification, CanonStateNotificationSender, CanonStateNotifications, ChainInfoTracker,
};
use alloy_consensus::transaction::TransactionMeta;
use alloy_eips::BlockNumHash;
use alloy_primitives::{map::B256Map, BlockNumber, TxHash, B256};
use parking_lot::RwLock;
use reth_chainspec::ChainInfo;
use reth_ethereum_primitives::EthPrimitives;
use reth_execution_types::RecoveredBlockAndExecutionOutput;
use reth_metrics::{metrics::Gauge, Metrics};
use reth_primitives_traits::{
    BlockBody as _, NodePrimitives, RecoveredBlock, SealedBlock, SealedHeader,
};
use std::{collections::BTreeMap, sync::Arc, time::Instant};
use tokio::sync::{broadcast, watch};

/// Size of the broadcast channel used to notify canonical state events.
const CANON_STATE_NOTIFICATION_CHANNEL_SIZE: usize = 256;

/// Metrics for the in-memory state.
#[derive(Metrics)]
#[metrics(scope = "blockchain_tree.in_mem_state")]
pub(crate) struct InMemoryStateMetrics {
    /// The block number of the earliest block in the in-memory state.
    pub(crate) earliest_block: Gauge,
    /// The block number of the latest block in the in-memory state.
    pub(crate) latest_block: Gauge,
    /// The number of blocks in the in-memory state.
    pub(crate) num_blocks: Gauge,
}

/// The numbers the metrics of the in-memory state are recorded from.
#[derive(Debug, Clone, Copy)]
pub(super) struct InMemoryStateStats {
    /// The block number of the earliest block in the in-memory state.
    pub(super) earliest: Option<BlockNumber>,
    /// The block number of the latest block in the in-memory state.
    pub(super) latest: Option<BlockNumber>,
    /// The number of blocks in the in-memory state.
    pub(super) count: usize,
}

/// Container type for in memory state data of the canonical chain.
///
/// This tracks blocks and their state that haven't been persisted to disk yet but are part of the
/// canonical chain that can be traced back to a canonical block on disk.
///
/// # Locking behavior on state updates
///
/// The blocks, the mapping of their numbers and the pending block are behind one lock. An update
/// holds its write lock for all of its changes, so that readers never observe partially updated
/// state where they are out of sync.
#[derive(Debug, Default)]
pub(crate) struct InMemoryState<N: NodePrimitives = EthPrimitives> {
    /// The canonical blocks that are not on disk yet and the pending block.
    blocks: RwLock<InMemoryBlocks<N>>,
    /// Metrics for the in-memory state.
    metrics: InMemoryStateMetrics,
}

impl<N: NodePrimitives> InMemoryState<N> {
    pub(crate) fn new(
        blocks: B256Map<Arc<BlockState<N>>>,
        numbers: BTreeMap<u64, B256>,
        pending: Option<BlockState<N>>,
    ) -> Self {
        let blocks = InMemoryBlocks::new(blocks, numbers, pending);
        let stats = blocks.stats();
        let this = Self { blocks: RwLock::new(blocks), metrics: Default::default() };
        this.update_metrics(stats);
        this
    }

    /// Applies `f` to the blocks under the write lock, and updates the metrics after releasing it.
    fn update<R>(&self, f: impl FnOnce(&mut InMemoryBlocks<N>) -> R) -> R {
        let (result, stats) = {
            let mut blocks = self.blocks.write();
            let result = f(&mut blocks);
            (result, blocks.stats())
        };
        self.update_metrics(stats);
        result
    }

    /// Update the metrics for the in-memory state.
    fn update_metrics(&self, stats: InMemoryStateStats) {
        if let Some(earliest_block_number) = stats.earliest {
            self.metrics.earliest_block.set(earliest_block_number as f64);
        }
        if let Some(latest_block_number) = stats.latest {
            self.metrics.latest_block.set(latest_block_number as f64);
        }
        self.metrics.num_blocks.set(stats.count as f64);
    }

    /// Returns the state for a given block hash.
    pub(crate) fn state_by_hash(&self, hash: B256) -> Option<Arc<BlockState<N>>> {
        self.blocks.read().canonical().get(&hash).cloned()
    }

    /// Returns the state for a given block number.
    pub(crate) fn state_by_number(&self, number: u64) -> Option<Arc<BlockState<N>>> {
        self.blocks.read().canonical().by_number(number).cloned()
    }

    /// Returns the hash for a specific block number
    pub(crate) fn hash_by_number(&self, number: u64) -> Option<B256> {
        self.blocks.read().canonical().hash_by_number(number)
    }

    /// Returns the current chain head state.
    pub(crate) fn head_state(&self) -> Option<Arc<BlockState<N>>> {
        self.blocks.read().canonical().head().cloned()
    }

    /// Returns the pending state corresponding to the current head plus one,
    /// from the payload received in newPayload that does not have a FCU yet.
    pub(crate) fn pending_state(&self) -> Option<BlockState<N>> {
        self.blocks.read().pending_state().cloned()
    }

    #[cfg(test)]
    fn block_count(&self) -> usize {
        self.blocks.read().canonical().len()
    }
}

/// Inner type to provide in memory state. It includes a chain tracker to be
/// advanced internally by the tree.
#[derive(Debug)]
pub(crate) struct CanonicalInMemoryStateInner<N: NodePrimitives> {
    /// Tracks certain chain information, such as the canonical head, safe head, and finalized
    /// head.
    pub(crate) chain_info_tracker: ChainInfoTracker<N>,
    /// Tracks blocks at the tip of the chain that have not been persisted to disk yet.
    pub(crate) in_memory_state: InMemoryState<N>,
    /// A broadcast stream that emits events when the canonical chain is updated.
    pub(crate) canon_state_notification_sender: CanonStateNotificationSender<N>,
}

impl<N: NodePrimitives> CanonicalInMemoryStateInner<N> {
    /// Clears all entries in the in memory state.
    fn clear(&self) {
        self.in_memory_state.update(InMemoryBlocks::clear);
    }
}

/// This type is responsible for providing the blocks, receipts, and state for
/// all canonical blocks not on disk yet and keeps track of the block range that
/// is in memory.
#[derive(Debug, Clone)]
pub struct CanonicalInMemoryState<N: NodePrimitives = EthPrimitives> {
    pub(crate) inner: Arc<CanonicalInMemoryStateInner<N>>,
}

impl<N: NodePrimitives> CanonicalInMemoryState<N> {
    /// Create a new in-memory state with the given blocks, numbers, pending state, and optional
    /// finalized header.
    pub fn new(
        blocks: B256Map<Arc<BlockState<N>>>,
        numbers: BTreeMap<u64, B256>,
        pending: Option<BlockState<N>>,
        finalized: Option<SealedHeader<N::BlockHeader>>,
        safe: Option<SealedHeader<N::BlockHeader>>,
    ) -> Self {
        let in_memory_state = InMemoryState::new(blocks, numbers, pending);
        let header = in_memory_state.head_state().map_or_else(SealedHeader::default, |state| {
            state.block_ref().recovered_block().clone_sealed_header()
        });
        let chain_info_tracker = ChainInfoTracker::new(header, finalized, safe);
        let (canon_state_notification_sender, _) =
            broadcast::channel(CANON_STATE_NOTIFICATION_CHANNEL_SIZE);

        Self {
            inner: Arc::new(CanonicalInMemoryStateInner {
                chain_info_tracker,
                in_memory_state,
                canon_state_notification_sender,
            }),
        }
    }

    /// Create an empty state.
    pub fn empty() -> Self {
        Self::new(B256Map::default(), BTreeMap::new(), None, None, None)
    }

    /// Create a new in memory state with the given local head and finalized header
    /// if it exists.
    pub fn with_head(
        head: SealedHeader<N::BlockHeader>,
        finalized: Option<SealedHeader<N::BlockHeader>>,
        safe: Option<SealedHeader<N::BlockHeader>>,
    ) -> Self {
        let chain_info_tracker = ChainInfoTracker::new(head, finalized, safe);
        let in_memory_state = InMemoryState::default();
        let (canon_state_notification_sender, _) =
            broadcast::channel(CANON_STATE_NOTIFICATION_CHANNEL_SIZE);
        let inner = CanonicalInMemoryStateInner {
            chain_info_tracker,
            in_memory_state,
            canon_state_notification_sender,
        };

        Self { inner: Arc::new(inner) }
    }

    /// Returns the block hash corresponding to the given number.
    pub fn hash_by_number(&self, number: u64) -> Option<B256> {
        self.inner.in_memory_state.hash_by_number(number)
    }

    /// Returns the header corresponding to the given hash.
    pub fn header_by_hash(&self, hash: B256) -> Option<SealedHeader<N::BlockHeader>> {
        self.state_by_hash(hash)
            .map(|block| block.block_ref().recovered_block().clone_sealed_header())
    }

    /// Clears all entries in the in memory state.
    pub fn clear_state(&self) {
        self.inner.clear()
    }

    /// Updates the pending block with the given block.
    ///
    /// Note: This assumes that the parent block of the pending block is canonical.
    pub fn set_pending_block(&self, pending: ExecutedBlock<N>) {
        self.inner.in_memory_state.update(|blocks| blocks.set_pending_block(pending));
    }

    /// Append new blocks to the in memory state.
    ///
    /// This removes all reorged blocks and appends the new blocks to the tracked chain and connects
    /// them to their parent blocks.
    fn update_blocks<I, R>(&self, new_blocks: I, reorged: R)
    where
        I: IntoIterator<Item = ExecutedBlock<N>>,
        R: IntoIterator<Item = ExecutedBlock<N>>,
    {
        self.inner.in_memory_state.update(|blocks| blocks.update_blocks(new_blocks, reorged));
    }

    /// Update the in memory state with the given chain update.
    pub fn update_chain(&self, new_chain: NewCanonicalChain<N>) {
        match new_chain {
            NewCanonicalChain::Commit { new } => {
                self.update_blocks(new, vec![]);
            }
            NewCanonicalChain::Reorg { new, old } => {
                self.update_blocks(new, old);
            }
        }
    }

    /// Removes blocks from the in memory state that are persisted to the given height.
    ///
    /// This will update the links between blocks and remove all blocks that are [..
    /// `persisted_height`].
    pub fn remove_persisted_blocks(&self, persisted_num_hash: BlockNumHash) {
        self.remove_persisted_blocks_until(persisted_num_hash, persisted_num_hash.number);
    }

    /// Removes blocks from the in-memory state through `remove_until` while still reporting the
    /// provided block as the persisted tip.
    pub fn remove_persisted_blocks_until(
        &self,
        persisted_num_hash: BlockNumHash,
        remove_until: BlockNumber,
    ) {
        self.set_persisted(persisted_num_hash);
        self.inner.in_memory_state.update(|blocks| {
            blocks.remove_persisted_blocks_until(persisted_num_hash, remove_until)
        });
    }

    /// Returns in memory state corresponding the given hash.
    pub fn state_by_hash(&self, hash: B256) -> Option<Arc<BlockState<N>>> {
        self.inner.in_memory_state.state_by_hash(hash)
    }

    /// Returns in memory state corresponding the block number.
    pub fn state_by_number(&self, number: u64) -> Option<Arc<BlockState<N>>> {
        self.inner.in_memory_state.state_by_number(number)
    }

    /// Returns the in memory head state.
    pub fn head_state(&self) -> Option<Arc<BlockState<N>>> {
        self.inner.in_memory_state.head_state()
    }

    /// Returns the in memory pending state.
    pub fn pending_state(&self) -> Option<BlockState<N>> {
        self.inner.in_memory_state.pending_state()
    }

    /// Returns the in memory pending `BlockNumHash`.
    pub fn pending_block_num_hash(&self) -> Option<BlockNumHash> {
        self.inner
            .in_memory_state
            .pending_state()
            .map(|state| BlockNumHash { number: state.number(), hash: state.hash() })
    }

    /// Returns the current `ChainInfo`.
    pub fn chain_info(&self) -> ChainInfo {
        self.inner.chain_info_tracker.chain_info()
    }

    /// Returns the latest canonical block number.
    pub fn get_canonical_block_number(&self) -> u64 {
        self.inner.chain_info_tracker.get_canonical_block_number()
    }

    /// Returns the `BlockNumHash` of the safe head.
    pub fn get_safe_num_hash(&self) -> Option<BlockNumHash> {
        self.inner.chain_info_tracker.get_safe_num_hash()
    }

    /// Returns the `BlockNumHash` of the finalized head.
    pub fn get_finalized_num_hash(&self) -> Option<BlockNumHash> {
        self.inner.chain_info_tracker.get_finalized_num_hash()
    }

    /// Hook for new fork choice update.
    pub fn on_forkchoice_update_received(&self) {
        self.inner.chain_info_tracker.on_forkchoice_update_received();
    }

    /// Returns the timestamp of the last received update.
    pub fn last_received_update_timestamp(&self) -> Option<Instant> {
        self.inner.chain_info_tracker.last_forkchoice_update_received_at()
    }

    /// Canonical head setter.
    pub fn set_canonical_head(&self, header: SealedHeader<N::BlockHeader>) {
        self.inner.chain_info_tracker.set_canonical_head(header);
    }

    /// Safe head setter.
    pub fn set_safe(&self, header: SealedHeader<N::BlockHeader>) {
        self.inner.chain_info_tracker.set_safe(header);
    }

    /// Finalized head setter.
    pub fn set_finalized(&self, header: SealedHeader<N::BlockHeader>) {
        self.inner.chain_info_tracker.set_finalized(header);
    }

    /// Persisted block setter.
    pub fn set_persisted(&self, num_hash: BlockNumHash) {
        self.inner.chain_info_tracker.set_persisted(num_hash);
    }

    /// Canonical head getter.
    pub fn get_canonical_head(&self) -> SealedHeader<N::BlockHeader> {
        self.inner.chain_info_tracker.get_canonical_head()
    }

    /// Finalized header getter.
    pub fn get_finalized_header(&self) -> Option<SealedHeader<N::BlockHeader>> {
        self.inner.chain_info_tracker.get_finalized_header()
    }

    /// Safe header getter.
    pub fn get_safe_header(&self) -> Option<SealedHeader<N::BlockHeader>> {
        self.inner.chain_info_tracker.get_safe_header()
    }

    /// Persisted block `BlockNumHash` getter.
    pub fn get_persisted_num_hash(&self) -> Option<BlockNumHash> {
        self.inner.chain_info_tracker.get_persisted_num_hash()
    }

    /// Returns the `SealedHeader` corresponding to the pending state.
    pub fn pending_sealed_header(&self) -> Option<SealedHeader<N::BlockHeader>> {
        self.pending_state().map(|h| h.block_ref().recovered_block().clone_sealed_header())
    }

    /// Returns the `Header` corresponding to the pending state.
    pub fn pending_header(&self) -> Option<N::BlockHeader> {
        self.pending_sealed_header().map(|sealed_header| sealed_header.unseal())
    }

    /// Returns the `SealedBlock` corresponding to the pending state.
    pub fn pending_block(&self) -> Option<SealedBlock<N::Block>> {
        self.pending_state()
            .map(|block_state| block_state.block_ref().recovered_block().sealed_block().clone())
    }

    /// Returns the `RecoveredBlock` corresponding to the pending state.
    pub fn pending_recovered_block(&self) -> Option<Arc<RecoveredBlock<N::Block>>> {
        self.pending_state().map(|block_state| Arc::clone(&block_state.block_ref().recovered_block))
    }

    /// Returns the pending recovered block and its execution output, which contains the receipts.
    pub fn pending_block_and_receipts(
        &self,
    ) -> Option<RecoveredBlockAndExecutionOutput<N::Block, N::Receipt>> {
        self.pending_state().map(|block_state| {
            RecoveredBlockAndExecutionOutput::new(
                Arc::clone(&block_state.block_ref().recovered_block),
                Arc::clone(&block_state.block_ref().execution_output),
            )
        })
    }

    /// Subscribe to new blocks events.
    pub fn subscribe_canon_state(&self) -> CanonStateNotifications<N> {
        self.inner.canon_state_notification_sender.subscribe()
    }

    /// Subscribe to new safe block events.
    pub fn subscribe_safe_block(&self) -> watch::Receiver<Option<SealedHeader<N::BlockHeader>>> {
        self.inner.chain_info_tracker.subscribe_safe_block()
    }

    /// Subscribe to new finalized block events.
    pub fn subscribe_finalized_block(
        &self,
    ) -> watch::Receiver<Option<SealedHeader<N::BlockHeader>>> {
        self.inner.chain_info_tracker.subscribe_finalized_block()
    }

    /// Subscribe to new persisted block events.
    pub fn subscribe_persisted_block(&self) -> watch::Receiver<Option<BlockNumHash>> {
        self.inner.chain_info_tracker.subscribe_persisted_block()
    }

    /// Attempts to send a new [`CanonStateNotification`] to all active Receiver handles.
    pub fn notify_canon_state(&self, event: CanonStateNotification<N>) {
        self.inner.canon_state_notification_sender.send(event).ok();
    }

    /// Returns an iterator over all __canonical blocks__ in the in-memory state, from newest to
    /// oldest (highest to lowest).
    ///
    /// This iterator contains a snapshot of the in-memory state at the time of the call.
    pub fn canonical_chain(&self) -> impl Iterator<Item = Arc<BlockState<N>>> {
        self.inner.in_memory_state.head_state().into_iter().flat_map(|head| head.iter())
    }

    /// Returns [`SignedTransaction`] type for the given `TxHash` if found.
    ///
    /// [`SignedTransaction`]: reth_primitives_traits::SignedTransaction
    pub fn transaction_by_hash(&self, hash: TxHash) -> Option<N::SignedTx> {
        for block_state in self.canonical_chain() {
            if let Some(tx) =
                block_state.block_ref().recovered_block().body().transaction_by_hash(&hash)
            {
                return Some(tx.clone())
            }
        }
        None
    }

    /// Returns a tuple with [`SignedTransaction`] type and [`TransactionMeta`] for the
    /// given [`TxHash`] if found.
    ///
    /// [`SignedTransaction`]: reth_primitives_traits::SignedTransaction
    pub fn transaction_by_hash_with_meta(
        &self,
        tx_hash: TxHash,
    ) -> Option<(N::SignedTx, TransactionMeta)> {
        for block_state in self.canonical_chain() {
            if let Some(indexed) = block_state.find_indexed(tx_hash) {
                return Some((indexed.tx().clone(), indexed.meta()));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::TestBlockBuilder;
    use rand::Rng;

    fn create_mock_state(
        test_block_builder: &mut TestBlockBuilder<EthPrimitives>,
        block_number: u64,
        parent_hash: B256,
    ) -> BlockState {
        BlockState::new(
            test_block_builder.get_executed_block_with_number(block_number, parent_hash),
        )
    }

    #[test]
    fn test_in_memory_state_impl_state_by_hash() {
        let mut state_by_hash = B256Map::default();
        let number = rand::rng().random::<u64>();
        let mut test_block_builder: TestBlockBuilder = TestBlockBuilder::default();
        let state = Arc::new(create_mock_state(&mut test_block_builder, number, B256::random()));
        state_by_hash.insert(state.hash(), state.clone());

        let in_memory_state = InMemoryState::new(state_by_hash, BTreeMap::new(), None);

        assert_eq!(in_memory_state.state_by_hash(state.hash()), Some(state));
        assert_eq!(in_memory_state.state_by_hash(B256::random()), None);
    }

    #[test]
    fn test_in_memory_state_impl_state_by_number() {
        let mut state_by_hash = B256Map::default();
        let mut hash_by_number = BTreeMap::new();

        let number = rand::rng().random::<u64>();
        let mut test_block_builder: TestBlockBuilder = TestBlockBuilder::default();
        let state = Arc::new(create_mock_state(&mut test_block_builder, number, B256::random()));
        let hash = state.hash();

        state_by_hash.insert(hash, state.clone());
        hash_by_number.insert(number, hash);

        let in_memory_state = InMemoryState::new(state_by_hash, hash_by_number, None);

        assert_eq!(in_memory_state.state_by_number(number), Some(state));
        assert_eq!(in_memory_state.state_by_number(number + 1), None);
    }

    #[test]
    fn test_in_memory_state_impl_head_state() {
        let mut state_by_hash = B256Map::default();
        let mut hash_by_number = BTreeMap::new();
        let mut test_block_builder: TestBlockBuilder = TestBlockBuilder::default();
        let state1 = Arc::new(create_mock_state(&mut test_block_builder, 1, B256::random()));
        let hash1 = state1.hash();
        let state2 = Arc::new(create_mock_state(&mut test_block_builder, 2, hash1));
        let hash2 = state2.hash();
        hash_by_number.insert(1, hash1);
        hash_by_number.insert(2, hash2);
        state_by_hash.insert(hash1, state1);
        state_by_hash.insert(hash2, state2);

        let in_memory_state = InMemoryState::new(state_by_hash, hash_by_number, None);
        let head_state = in_memory_state.head_state().unwrap();

        assert_eq!(head_state.hash(), hash2);
        assert_eq!(head_state.number(), 2);
    }

    #[test]
    fn test_in_memory_state_impl_pending_state() {
        let pending_number = rand::rng().random::<u64>();
        let mut test_block_builder: TestBlockBuilder = TestBlockBuilder::default();
        let pending_state =
            create_mock_state(&mut test_block_builder, pending_number, B256::random());
        let pending_hash = pending_state.hash();

        let in_memory_state =
            InMemoryState::new(B256Map::default(), BTreeMap::new(), Some(pending_state));

        let result = in_memory_state.pending_state();
        assert!(result.is_some());
        let actual_pending_state = result.unwrap();
        assert_eq!(actual_pending_state.block.recovered_block().hash(), pending_hash);
        assert_eq!(actual_pending_state.block.recovered_block().number, pending_number);
    }

    #[test]
    fn test_in_memory_state_impl_no_pending_state() {
        let in_memory_state: InMemoryState =
            InMemoryState::new(B256Map::default(), BTreeMap::new(), None);

        assert_eq!(in_memory_state.pending_state(), None);
    }

    #[test]
    fn test_in_memory_state_chain_update() {
        let state: CanonicalInMemoryState = CanonicalInMemoryState::empty();
        let mut test_block_builder: TestBlockBuilder = TestBlockBuilder::default();
        let block1 = test_block_builder.get_executed_block_with_number(0, B256::random());
        let block2 = test_block_builder.get_executed_block_with_number(0, B256::random());
        let chain = NewCanonicalChain::Commit { new: vec![block1.clone()] };
        state.update_chain(chain);
        assert_eq!(
            state.head_state().unwrap().block_ref().recovered_block().hash(),
            block1.recovered_block().hash()
        );
        assert_eq!(
            state.state_by_number(0).unwrap().block_ref().recovered_block().hash(),
            block1.recovered_block().hash()
        );

        let chain = NewCanonicalChain::Reorg { new: vec![block2.clone()], old: vec![block1] };
        state.update_chain(chain);
        assert_eq!(
            state.head_state().unwrap().block_ref().recovered_block().hash(),
            block2.recovered_block().hash()
        );
        assert_eq!(
            state.state_by_number(0).unwrap().block_ref().recovered_block().hash(),
            block2.recovered_block().hash()
        );

        assert_eq!(state.inner.in_memory_state.block_count(), 1);
    }

    #[test]
    fn test_in_memory_state_set_pending_block() {
        let state: CanonicalInMemoryState = CanonicalInMemoryState::empty();
        let mut test_block_builder: TestBlockBuilder = TestBlockBuilder::default();

        // First random block
        let block1 = test_block_builder.get_executed_block_with_number(0, B256::random());

        // Second block with parent hash of the first block
        let block2 =
            test_block_builder.get_executed_block_with_number(1, block1.recovered_block().hash());

        // Commit the two blocks
        let chain = NewCanonicalChain::Commit { new: vec![block1.clone(), block2.clone()] };
        state.update_chain(chain);

        // Assert that the pending state is None before setting it
        assert!(state.pending_state().is_none());

        // Set the pending block
        state.set_pending_block(block2.clone());

        // Check the pending state
        assert_eq!(
            state.pending_state().unwrap(),
            BlockState::with_parent(block2.clone(), Some(Arc::new(BlockState::new(block1))))
        );

        // Check the pending block
        assert_eq!(state.pending_block().unwrap(), block2.recovered_block().sealed_block().clone());

        // Check the pending block number and hash
        assert_eq!(
            state.pending_block_num_hash().unwrap(),
            BlockNumHash { number: 1, hash: block2.recovered_block().hash() }
        );

        // Check the pending header
        assert_eq!(state.pending_header().unwrap(), block2.recovered_block().header().clone());

        // Check the pending sealed header
        assert_eq!(
            state.pending_sealed_header().unwrap(),
            block2.recovered_block().clone_sealed_header()
        );

        // Check the pending block with senders
        let pending_block = state.pending_recovered_block().unwrap();
        assert!(Arc::ptr_eq(&pending_block, &block2.recovered_block));

        // Check the pending block and receipts
        let pending = state.pending_block_and_receipts().unwrap();
        assert!(Arc::ptr_eq(pending.block(), &block2.recovered_block));
        assert!(Arc::ptr_eq(pending.execution_output(), &block2.execution_output));
    }

    #[test]
    fn test_canonical_in_memory_state_canonical_chain_empty() {
        let state: CanonicalInMemoryState = CanonicalInMemoryState::empty();
        assert!(state.canonical_chain().next().is_none());
    }

    #[test]
    fn test_canonical_in_memory_state_canonical_chain_single_block() {
        let block = TestBlockBuilder::eth().get_executed_block_with_number(1, B256::random());
        let hash = block.recovered_block().hash();
        let mut blocks = B256Map::default();
        blocks.insert(hash, Arc::new(BlockState::new(block)));
        let mut numbers = BTreeMap::new();
        numbers.insert(1, hash);

        let state = CanonicalInMemoryState::new(blocks, numbers, None, None, None);
        let chain: Vec<_> = state.canonical_chain().collect();

        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0].number(), 1);
        assert_eq!(chain[0].hash(), hash);
    }

    #[test]
    fn test_canonical_in_memory_state_canonical_chain_multiple_blocks() {
        let mut parent_hash = B256::random();
        let mut block_builder = TestBlockBuilder::eth();
        let state: CanonicalInMemoryState = CanonicalInMemoryState::empty();

        for i in 1..=3 {
            let block = block_builder.get_executed_block_with_number(i, parent_hash);
            let hash = block.recovered_block().hash();
            state.update_blocks(Some(block), None);
            parent_hash = hash;
        }

        let chain: Vec<_> = state.canonical_chain().collect();

        assert_eq!(chain.len(), 3);
        assert_eq!(chain[0].number(), 3);
        assert_eq!(chain[1].number(), 2);
        assert_eq!(chain[2].number(), 1);
    }

    // ensures the pending block is not part of the canonical chain
    #[test]
    fn test_canonical_in_memory_state_canonical_chain_with_pending_block() {
        let mut parent_hash = B256::random();
        let mut block_builder = TestBlockBuilder::<EthPrimitives>::eth();
        let state: CanonicalInMemoryState = CanonicalInMemoryState::empty();

        for i in 1..=2 {
            let block = block_builder.get_executed_block_with_number(i, parent_hash);
            let hash = block.recovered_block().hash();
            state.update_blocks(Some(block), None);
            parent_hash = hash;
        }

        let pending_block = block_builder.get_executed_block_with_number(3, parent_hash);
        state.set_pending_block(pending_block);
        let chain: Vec<_> = state.canonical_chain().collect();

        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0].number(), 2);
        assert_eq!(chain[1].number(), 1);
    }

    #[test]
    fn test_canonical_in_memory_state_remove_persisted_blocks() {
        let state: CanonicalInMemoryState = CanonicalInMemoryState::empty();
        let blocks = TestBlockBuilder::eth().get_executed_blocks(1..6).collect::<Vec<_>>();
        let num_hash = |number: usize| blocks[number - 1].recovered_block().num_hash();
        // The numbers of the blocks a state is linked to, itself included.
        let chain = |state: &BlockState| state.chain().map(BlockState::number).collect::<Vec<_>>();

        state.update_chain(NewCanonicalChain::Commit { new: blocks[..4].to_vec() });
        state.set_pending_block(blocks[4].clone());

        // A persisted block that is not tracked is recorded, but removes nothing.
        let untracked = BlockNumHash::new(2, B256::random());
        state.remove_persisted_blocks(untracked);
        assert_eq!(state.get_persisted_num_hash(), Some(untracked));
        assert_eq!(chain(&state.head_state().unwrap()), [4, 3, 2, 1]);
        assert_eq!(chain(&state.pending_state().unwrap()), [5, 4, 3, 2, 1]);

        // Only the blocks through the given number are removed. The blocks that stay and the
        // pending block are no longer linked to the removed ones.
        state.remove_persisted_blocks_until(num_hash(2), 1);
        assert_eq!(state.get_persisted_num_hash(), Some(num_hash(2)));
        assert_eq!(state.hash_by_number(1), None);
        assert_eq!(state.state_by_hash(num_hash(1).hash), None);
        assert_eq!(chain(&state.state_by_number(2).unwrap()), [2]);
        assert_eq!(chain(&state.head_state().unwrap()), [4, 3, 2]);
        assert_eq!(chain(&state.pending_state().unwrap()), [5, 4, 3, 2]);

        // No block above the persisted block is removed.
        state.remove_persisted_blocks_until(num_hash(3), 4);
        assert_eq!(state.hash_by_number(3), None);
        assert_eq!(chain(&state.head_state().unwrap()), [4]);
        assert_eq!(chain(&state.pending_state().unwrap()), [5, 4]);

        // The pending block stays when the last canonical block is removed.
        state.remove_persisted_blocks(num_hash(4));
        assert_eq!(state.head_state(), None);
        assert_eq!(state.inner.in_memory_state.block_count(), 0);
        assert_eq!(chain(&state.pending_state().unwrap()), [5]);

        state.clear_state();
        assert_eq!(state.pending_state(), None);
    }
}

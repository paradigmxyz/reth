//! Types for tracking the canonical chain state in memory.

use crate::ChainInfoTracker;
use parking_lot::RwLock;
use reth_chainspec::ChainInfo;
use reth_execution_types::ExecutionOutcome;
use reth_primitives::{Address, BlockNumHash, Receipts, SealedBlock, SealedHeader, B256};
use reth_trie::{updates::TrieUpdates, HashedPostState};
use std::{collections::HashMap, sync::Arc, time::Instant};

/// Container type for in memory state data.
#[derive(Debug, Default)]
pub(crate) struct InMemoryState {
    blocks: RwLock<HashMap<B256, Arc<BlockState>>>,
    numbers: RwLock<HashMap<u64, B256>>,
    pending: RwLock<Option<BlockState>>,
}

impl InMemoryState {
    pub(crate) const fn new(
        blocks: HashMap<B256, Arc<BlockState>>,
        numbers: HashMap<u64, B256>,
        pending: Option<BlockState>,
    ) -> Self {
        Self {
            blocks: RwLock::new(blocks),
            numbers: RwLock::new(numbers),
            pending: RwLock::new(pending),
        }
    }

    /// Returns the state for a given block hash.
    pub(crate) fn state_by_hash(&self, hash: B256) -> Option<Arc<BlockState>> {
        self.blocks.read().get(&hash).cloned()
    }

    /// Returns the state for a given block number.
    pub(crate) fn state_by_number(&self, number: u64) -> Option<Arc<BlockState>> {
        self.numbers.read().get(&number).and_then(|hash| self.blocks.read().get(hash).cloned())
    }

    /// Returns the current chain head state.
    pub(crate) fn head_state(&self) -> Option<Arc<BlockState>> {
        self.numbers
            .read()
            .iter()
            .max_by_key(|(&number, _)| number)
            .and_then(|(_, hash)| self.blocks.read().get(hash).cloned())
    }

    /// Returns the pending state corresponding to the current head plus one,
    /// from the payload received in newPayload that does not have a FCU yet.
    pub(crate) fn pending_state(&self) -> Option<Arc<BlockState>> {
        self.pending.read().as_ref().map(|state| Arc::new(BlockState(state.0.clone())))
    }
}

/// Inner type to provide in memory state. It includes a chain tracker to be
/// advanced internally by the tree.
#[derive(Debug)]
pub(crate) struct CanonicalInMemoryStateInner {
    pub(crate) chain_info_tracker: ChainInfoTracker,
    pub(crate) in_memory_state: InMemoryState,
}

/// This type is responsible for providing the blocks, receipts, and state for
/// all canonical blocks not on disk yet and keeps track of the block range that
/// is in memory.
#[derive(Debug, Clone)]
pub struct CanonicalInMemoryState {
    pub(crate) inner: Arc<CanonicalInMemoryStateInner>,
}

impl CanonicalInMemoryState {
    /// Create a new in memory state with the given blocks, numbers, and pending state.
    pub fn new(
        blocks: HashMap<B256, Arc<BlockState>>,
        numbers: HashMap<u64, B256>,
        pending: Option<BlockState>,
    ) -> Self {
        let in_memory_state = InMemoryState::new(blocks, numbers, pending);
        let head_state = in_memory_state.head_state();
        let header = match head_state {
            Some(state) => state.block().block().header.clone(),
            None => SealedHeader::default(),
        };
        let chain_info_tracker = ChainInfoTracker::new(header);
        let inner = CanonicalInMemoryStateInner { chain_info_tracker, in_memory_state };

        Self { inner: Arc::new(inner) }
    }

    /// Create a new in memory state with the given local head.
    pub fn with_head(head: SealedHeader) -> Self {
        let chain_info_tracker = ChainInfoTracker::new(head);
        let in_memory_state = InMemoryState::default();
        let inner = CanonicalInMemoryStateInner { chain_info_tracker, in_memory_state };

        Self { inner: Arc::new(inner) }
    }

    /// Returns in memory state corresponding the given hash.
    pub fn state_by_hash(&self, hash: B256) -> Option<Arc<BlockState>> {
        self.inner.in_memory_state.state_by_hash(hash)
    }

    /// Returns in memory state corresponding the block number.
    pub fn state_by_number(&self, number: u64) -> Option<Arc<BlockState>> {
        self.inner.in_memory_state.state_by_number(number)
    }

    /// Returns the in memory head state.
    pub fn head_state(&self) -> Option<Arc<BlockState>> {
        self.inner.in_memory_state.head_state()
    }

    /// Returns the in memory pending state.
    pub fn pending_state(&self) -> Option<Arc<BlockState>> {
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

    /// Hook for transition configuration exchanged.
    pub fn on_transition_configuration_exchanged(&self) {
        self.inner.chain_info_tracker.on_transition_configuration_exchanged();
    }

    /// Returns the timepstamp of the last transition configuration exchanged,
    pub fn last_exchanged_transition_configuration_timestamp(&self) -> Option<Instant> {
        self.inner.chain_info_tracker.last_transition_configuration_exchanged_at()
    }

    /// Canonical head setter.
    pub fn set_canonical_head(&self, header: SealedHeader) {
        self.inner.chain_info_tracker.set_canonical_head(header);
    }

    /// Safe head setter.
    pub fn set_safe(&self, header: SealedHeader) {
        self.inner.chain_info_tracker.set_safe(header);
    }

    /// Finalized head setter.
    pub fn set_finalized(&self, header: SealedHeader) {
        self.inner.chain_info_tracker.set_finalized(header);
    }

    /// Canonical head getter.
    pub fn get_canonical_head(&self) -> SealedHeader {
        self.inner.chain_info_tracker.get_canonical_head()
    }

    /// Finalized header getter.
    pub fn get_finalized_header(&self) -> Option<SealedHeader> {
        self.inner.chain_info_tracker.get_finalized_header()
    }

    /// Safe header getter.
    pub fn get_safe_header(&self) -> Option<SealedHeader> {
        self.inner.chain_info_tracker.get_safe_header()
    }
}

/// State after applying the given block.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct BlockState(pub ExecutedBlock);

#[allow(dead_code)]
impl BlockState {
    /// `BlockState` constructor.
    pub const fn new(executed_block: ExecutedBlock) -> Self {
        Self(executed_block)
    }

    pub(crate) fn block(&self) -> ExecutedBlock {
        self.0.clone()
    }

    pub(crate) fn hash(&self) -> B256 {
        self.0.block().hash()
    }

    pub(crate) fn number(&self) -> u64 {
        self.0.block().number
    }

    pub(crate) fn state_root(&self) -> B256 {
        self.0.block().header.state_root
    }

    pub(crate) fn receipts(&self) -> &Receipts {
        &self.0.execution_outcome().receipts
    }
}

/// Represents an executed block stored in-memory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecutedBlock {
    /// Sealed block the rest of fields refer to.
    pub block: Arc<SealedBlock>,
    /// Block's senders.
    pub senders: Arc<Vec<Address>>,
    /// Block's execution outcome.
    pub execution_output: Arc<ExecutionOutcome>,
    /// Block's hashedst state.
    pub hashed_state: Arc<HashedPostState>,
    /// Trie updates that result of applying the block.
    pub trie: Arc<TrieUpdates>,
}

impl ExecutedBlock {
    /// `ExecutedBlock` constructor.
    pub const fn new(
        block: Arc<SealedBlock>,
        senders: Arc<Vec<Address>>,
        execution_output: Arc<ExecutionOutcome>,
        hashed_state: Arc<HashedPostState>,
        trie: Arc<TrieUpdates>,
    ) -> Self {
        Self { block, senders, execution_output, hashed_state, trie }
    }

    /// Returns a reference to the executed block.
    pub fn block(&self) -> &SealedBlock {
        &self.block
    }

    /// Returns a reference to the block's senders
    pub fn senders(&self) -> &Vec<Address> {
        &self.senders
    }

    /// Returns a reference to the block's execution outcome
    pub fn execution_outcome(&self) -> &ExecutionOutcome {
        &self.execution_output
    }

    /// Returns a reference to the hashed state result of the execution outcome
    pub fn hashed_state(&self) -> &HashedPostState {
        &self.hashed_state
    }

    /// Returns a reference to the trie updates for the block
    pub fn trie_updates(&self) -> &TrieUpdates {
        &self.trie
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{get_executed_block_with_number, get_executed_block_with_receipts};
    use rand::Rng;
    use reth_primitives::Receipt;

    fn create_mock_state(block_number: u64) -> BlockState {
        BlockState::new(get_executed_block_with_number(block_number))
    }

    #[tokio::test]
    async fn test_in_memory_state_impl_state_by_hash() {
        let mut state_by_hash = HashMap::new();
        let number = rand::thread_rng().gen::<u64>();
        let state = Arc::new(create_mock_state(number));
        state_by_hash.insert(state.hash(), state.clone());

        let in_memory_state = InMemoryState::new(state_by_hash, HashMap::new(), None);

        assert_eq!(in_memory_state.state_by_hash(state.hash()), Some(state));
        assert_eq!(in_memory_state.state_by_hash(B256::random()), None);
    }

    #[tokio::test]
    async fn test_in_memory_state_impl_state_by_number() {
        let mut state_by_hash = HashMap::new();
        let mut hash_by_number = HashMap::new();

        let number = rand::thread_rng().gen::<u64>();
        let state = Arc::new(create_mock_state(number));
        let hash = state.hash();

        state_by_hash.insert(hash, state.clone());
        hash_by_number.insert(number, hash);

        let in_memory_state = InMemoryState::new(state_by_hash, hash_by_number, None);

        assert_eq!(in_memory_state.state_by_number(number), Some(state));
        assert_eq!(in_memory_state.state_by_number(number + 1), None);
    }

    #[tokio::test]
    async fn test_in_memory_state_impl_head_state() {
        let mut state_by_hash = HashMap::new();
        let mut hash_by_number = HashMap::new();
        let state1 = Arc::new(create_mock_state(1));
        let state2 = Arc::new(create_mock_state(2));
        let hash1 = state1.hash();
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

    #[tokio::test]
    async fn test_in_memory_state_impl_pending_state() {
        let pending_number = rand::thread_rng().gen::<u64>();
        let pending_state = create_mock_state(pending_number);
        let pending_hash = pending_state.hash();

        let in_memory_state =
            InMemoryState::new(HashMap::new(), HashMap::new(), Some(pending_state));

        let result = in_memory_state.pending_state();
        assert!(result.is_some());
        let actual_pending_state = result.unwrap();
        assert_eq!(actual_pending_state.0.block().hash(), pending_hash);
        assert_eq!(actual_pending_state.0.block().number, pending_number);
    }

    #[tokio::test]
    async fn test_in_memory_state_impl_no_pending_state() {
        let in_memory_state = InMemoryState::new(HashMap::new(), HashMap::new(), None);

        assert_eq!(in_memory_state.pending_state(), None);
    }

    #[tokio::test]
    async fn test_state_new() {
        let number = rand::thread_rng().gen::<u64>();
        let block = get_executed_block_with_number(number);

        let state = BlockState::new(block.clone());

        assert_eq!(state.block(), block);
    }

    #[tokio::test]
    async fn test_state_block() {
        let number = rand::thread_rng().gen::<u64>();
        let block = get_executed_block_with_number(number);

        let state = BlockState::new(block.clone());

        assert_eq!(state.block(), block);
    }

    #[tokio::test]
    async fn test_state_hash() {
        let number = rand::thread_rng().gen::<u64>();
        let block = get_executed_block_with_number(number);

        let state = BlockState::new(block.clone());

        assert_eq!(state.hash(), block.block().hash());
    }

    #[tokio::test]
    async fn test_state_number() {
        let number = rand::thread_rng().gen::<u64>();
        let block = get_executed_block_with_number(number);

        let state = BlockState::new(block);

        assert_eq!(state.number(), number);
    }

    #[tokio::test]
    async fn test_state_state_root() {
        let number = rand::thread_rng().gen::<u64>();
        let block = get_executed_block_with_number(number);

        let state = BlockState::new(block.clone());

        assert_eq!(state.state_root(), block.block().state_root);
    }

    #[tokio::test]
    async fn test_state_receipts() {
        let receipts = Receipts { receipt_vec: vec![vec![Some(Receipt::default())]] };

        let block = get_executed_block_with_receipts(receipts.clone());

        let state = BlockState::new(block);

        assert_eq!(state.receipts(), &receipts);
    }
}

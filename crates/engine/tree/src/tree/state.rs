//! Types for tracking the canonical chain state in memory.

use crate::tree::ExecutedBlock;
use parking_lot::RwLock;
use reth_primitives::{Receipts, SealedHeader, B256};
use reth_provider::providers::ChainInfoTracker;
use std::{collections::HashMap, sync::Arc};

/// Container type for in memory state data.
#[derive(Debug, Default)]
pub struct InMemoryStateImpl {
    blocks: RwLock<HashMap<B256, Arc<BlockState>>>,
    numbers: RwLock<HashMap<u64, B256>>,
    pending: RwLock<Option<BlockState>>,
}

impl InMemoryStateImpl {
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
}

impl InMemoryState for InMemoryStateImpl {
    fn state_by_hash(&self, hash: B256) -> Option<Arc<BlockState>> {
        self.blocks.read().get(&hash).cloned()
    }

    fn state_by_number(&self, number: u64) -> Option<Arc<BlockState>> {
        self.numbers.read().get(&number).and_then(|hash| self.blocks.read().get(hash).cloned())
    }

    fn head_state(&self) -> Option<Arc<BlockState>> {
        self.numbers
            .read()
            .iter()
            .max_by_key(|(&number, _)| number)
            .and_then(|(_, hash)| self.blocks.read().get(hash).cloned())
    }

    fn pending_state(&self) -> Option<Arc<BlockState>> {
        self.pending.read().as_ref().map(|state| Arc::new(BlockState(state.0.clone())))
    }
}

/// Inner type to provide in memory state. It includes a chain tracker to be
/// advanced internally by the tree.
#[derive(Debug)]
pub(crate) struct CanonicalInMemoryStateInner {
    pub(crate) chain_info_tracker: ChainInfoTracker,
    pub(crate) in_memory_state: InMemoryStateImpl,
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
        let in_memory_state = InMemoryStateImpl::new(blocks, numbers, pending);
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
        let in_memory_state = InMemoryStateImpl::default();
        let inner = CanonicalInMemoryStateInner { chain_info_tracker, in_memory_state };

        Self { inner: Arc::new(inner) }
    }
}

impl InMemoryState for CanonicalInMemoryState {
    fn state_by_hash(&self, hash: B256) -> Option<Arc<BlockState>> {
        self.inner.in_memory_state.state_by_hash(hash)
    }

    fn state_by_number(&self, number: u64) -> Option<Arc<BlockState>> {
        self.inner.in_memory_state.state_by_number(number)
    }

    fn head_state(&self) -> Option<Arc<BlockState>> {
        self.inner.in_memory_state.head_state()
    }

    fn pending_state(&self) -> Option<Arc<BlockState>> {
        self.inner.in_memory_state.pending_state()
    }
}

/// Represents the tree state kept in memory.
pub trait InMemoryState: Send + Sync {
    /// Returns the state for a given block hash.
    fn state_by_hash(&self, hash: B256) -> Option<Arc<BlockState>>;
    /// Returns the state for a given block number.
    fn state_by_number(&self, number: u64) -> Option<Arc<BlockState>>;
    /// Returns the current chain head state.
    fn head_state(&self) -> Option<Arc<BlockState>>;
    /// Returns the pending state corresponding to the current head plus one,
    /// from the payload received in newPayload that does not have a FCU yet.
    fn pending_state(&self) -> Option<Arc<BlockState>>;
}

/// State after applying the given block.
#[derive(Debug, PartialEq, Eq, Clone)]
pub struct BlockState(pub(crate) ExecutedBlock);

impl BlockState {
    pub(crate) const fn new(executed_block: ExecutedBlock) -> Self {
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

//! Helpers for exercising the overlay manager without an engine.
//!
//! The manager neither owns nor looks up in-memory blocks, so a test has to keep the blocks
//! itself and hand the manager the chains, the way [`CanonicalInMemoryState`] does in a running
//! node. [`TestOverlay`] pairs a manager with that store so tests can go on addressing blocks by
//! hash while the manager under test never sees one.

use crate::{OverlayBuilder, OverlayManager};
use alloy_eips::BlockNumHash;
use alloy_primitives::B256;
use reth_chain_state::{BlockState, CanonicalInMemoryState, ExecutedBlock, NewCanonicalChain};
use reth_ethereum_primitives::EthPrimitives;
#[cfg(feature = "rayon")]
use reth_tasks::WorkerPool;
use std::{ops::Deref, sync::Arc};

/// An [`OverlayManager`] plus the in-memory blocks a test tracks for it.
#[derive(Clone, Debug, Default)]
pub(crate) struct TestOverlay {
    manager: OverlayManager<EthPrimitives>,
    blocks: CanonicalInMemoryState<EthPrimitives>,
}

impl Deref for TestOverlay {
    type Target = OverlayManager<EthPrimitives>;

    fn deref(&self) -> &Self::Target {
        &self.manager
    }
}

impl TestOverlay {
    /// Creates a test overlay whose manager computes on `worker_pool`.
    #[cfg(feature = "rayon")]
    pub(crate) fn with_worker_pool(worker_pool: Arc<WorkerPool>) -> Self {
        Self { manager: OverlayManager::new(worker_pool), blocks: Default::default() }
    }

    /// Commits `block` to the canonical chain and hands the manager the resulting chain.
    pub(crate) fn insert_executed_block(
        &self,
        block: ExecutedBlock<EthPrimitives>,
    ) -> Arc<BlockState<EthPrimitives>> {
        let hash = block.recovered_block().hash();
        self.blocks.update_chain(NewCanonicalChain::Commit { new: vec![block] });
        let state = self.blocks.state_by_hash(hash).expect("block was just committed");
        self.manager.insert_block(Arc::clone(&state));
        state
    }

    /// Tracks `block` as a fork and hands the manager its chain.
    pub(crate) fn insert_fork(
        &self,
        block: ExecutedBlock<EthPrimitives>,
    ) -> Arc<BlockState<EthPrimitives>> {
        let state = self.blocks.insert_executed(block);
        self.manager.insert_block(Arc::clone(&state));
        state
    }

    /// Tracks `block` as the pending block and hands the manager its chain.
    pub(crate) fn set_pending_block(
        &self,
        block: ExecutedBlock<EthPrimitives>,
    ) -> Arc<BlockState<EthPrimitives>> {
        self.blocks.set_pending_block(block);
        let state = self.blocks.pending_state().expect("pending block was just set");
        self.manager.insert_block(Arc::clone(&state));
        state
    }

    /// Returns the chain ending at `hash`, canonical or not.
    pub(crate) fn state_for_hash(&self, hash: B256) -> Option<Arc<BlockState<EthPrimitives>>> {
        self.blocks.executed_state_by_hash(hash)
    }

    /// Builds an overlay builder for `hash` the way [`crate::OverlayBuilder`]'s callers do: from
    /// the tracked chain when the block is in memory, and from the database alone otherwise.
    pub(crate) fn overlay_builder_for_hash(&self, hash: B256) -> OverlayBuilder<EthPrimitives> {
        match self.state_for_hash(hash) {
            Some(state) => self.manager.overlay_builder_for_state(state),
            None => self.manager.overlay_builder_for_persisted(hash),
        }
    }

    /// Drops the blocks a persistence round made durable and prunes the manager's overlays, the
    /// way the engine's single trim does.
    pub(crate) fn remove_blocks_until(
        &self,
        persisted: BlockNumHash,
        removed: impl IntoIterator<Item = B256>,
    ) {
        self.blocks.remove_persisted_blocks_until(persisted, persisted.number);
        self.manager.remove_blocks(removed, persisted.number);
    }

    /// Drops non-canonical blocks and prunes the manager's overlays for them.
    pub(crate) fn remove_forks(
        &self,
        removed: impl IntoIterator<Item = B256> + Clone,
        state_trie_frontier: u64,
    ) {
        self.blocks.remove_executed_blocks(removed.clone());
        self.manager.remove_blocks(removed, state_trie_frontier);
    }
}

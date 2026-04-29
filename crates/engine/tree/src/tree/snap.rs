//! Snap sync helpers for [`EngineApiTreeHandler`].

use crate::backfill::{BackfillAction, BackfillSyncState};
use alloy_consensus::BlockHeader;
use alloy_eips::BlockNumHash;
use alloy_primitives::B256;
use reth_engine_primitives::ExecutionPayload;
use reth_evm::ConfigureEvm;
use reth_payload_primitives::{BuiltPayload, PayloadTypes};
use reth_primitives_traits::{NodePrimitives, SealedBlock};
use reth_provider::{
    BalProvider, BlockReader, ChangeSetReader, DatabaseProviderFactory, HashedPostStateProvider,
    StageCheckpointReader, StateProviderFactory, StateReader, StorageChangeSetReader,
    StorageSettingsCache,
};
use tokio::sync::mpsc::UnboundedSender;
use tracing::*;

use super::{
    error::InsertBlockFatalError, payload_validator::EngineValidator, EngineApiEvent,
    EngineApiTreeHandler, WaitForCaches,
};

/// Snap sync state owned by the engine tree.
#[derive(Debug, Default)]
pub(super) struct SnapTreeState {
    events_tx: Option<UnboundedSender<reth_engine_snap::SnapSyncEvent>>,
    fresh_node: bool,
}

impl SnapTreeState {
    /// Creates snap tree state for the given node freshness.
    pub(super) const fn new(fresh_node: bool) -> Self {
        Self { events_tx: None, fresh_node }
    }

    /// Returns true if this node started with no persisted blocks.
    pub(super) const fn is_fresh_node(&self) -> bool {
        self.fresh_node
    }

    /// Updates the fresh-node flag.
    pub(super) const fn set_fresh_node(&mut self, fresh_node: bool) {
        self.fresh_node = fresh_node;
    }

    /// Returns true if snap sync is currently receiving engine events.
    pub(super) const fn is_active(&self) -> bool {
        self.events_tx.is_some()
    }

    /// Starts forwarding engine events to snap sync.
    pub(super) fn start(&mut self, events_tx: UnboundedSender<reth_engine_snap::SnapSyncEvent>) {
        self.events_tx = Some(events_tx);
    }

    /// Marks snap sync as finished.
    pub(super) fn finish(&mut self) {
        self.events_tx = None;
        self.fresh_node = false;
    }

    fn events_tx(&self) -> Option<&UnboundedSender<reth_engine_snap::SnapSyncEvent>> {
        self.events_tx.as_ref()
    }
}

impl<N, P, T, V, C> EngineApiTreeHandler<N, P, T, V, C>
where
    N: NodePrimitives,
    P: DatabaseProviderFactory
        + BlockReader<Block = N::Block, Header = N::BlockHeader>
        + StateProviderFactory
        + StateReader<Receipt = N::Receipt>
        + HashedPostStateProvider
        + BalProvider
        + Clone
        + 'static,
    P::Provider: BlockReader<Block = N::Block, Header = N::BlockHeader>
        + StageCheckpointReader
        + ChangeSetReader
        + StorageChangeSetReader
        + StorageSettingsCache,
    C: ConfigureEvm<Primitives = N> + 'static,
    T: PayloadTypes<BuiltPayload: BuiltPayload<Primitives = N>>,
    V: EngineValidator<T> + WaitForCaches,
{
    /// Forwards a new block event to the snap sync orchestrator, if active.
    pub(super) fn forward_new_block_to_snap(&self, payload: &T::ExecutionData) {
        if let Some(events_tx) = self.snap.events_tx() {
            let _ = events_tx.send(reth_engine_snap::SnapSyncEvent::NewBlock {
                number: payload.block_number(),
                hash: payload.block_hash(),
                state_root: payload.state_root(),
                parent_hash: payload.parent_hash(),
                bal: payload.block_access_list().cloned(),
            });
        }
    }

    /// Forwards a new head event to the snap sync orchestrator, if active.
    pub(super) fn forward_head_to_snap(&self, head_hash: B256) {
        if let Some(events_tx) = self.snap.events_tx() {
            let _ = events_tx.send(reth_engine_snap::SnapSyncEvent::NewHead { head_hash });
        }
    }

    /// Forwards a downloaded block event to the snap sync orchestrator, if active.
    pub(super) fn forward_downloaded_block_to_snap(&self, block: &SealedBlock<N::Block>) {
        if let Some(events_tx) = self.snap.events_tx() {
            let _ = events_tx.send(reth_engine_snap::SnapSyncEvent::DownloadedBlock {
                number: block.number(),
                hash: block.hash(),
                state_root: block.header().state_root(),
                parent_hash: block.header().parent_hash(),
            });
        }
    }

    /// Handles snap sync completion.
    pub(super) fn on_snap_sync_finished(
        &mut self,
        outcome: reth_engine_snap::SnapSyncOutcome,
    ) -> Result<(), InsertBlockFatalError> {
        debug!(target: "engine::tree", synced_to = outcome.synced_to, %outcome.block_hash, "snap sync finished");
        self.backfill_sync_state = BackfillSyncState::Idle;
        self.snap.finish();

        let backfill_height = outcome.synced_to;
        let backfill_hash = outcome.block_hash;

        // Remove all blocks below the snap sync height
        self.state.buffer.remove_old_blocks(backfill_height);
        self.purge_timing_stats(backfill_height, None);
        self.canonical_in_memory_state.clear_state();

        // Update canonical head — try DB first, fall back to outcome data
        if let Ok(Some(new_head)) = self.provider.sealed_header(backfill_height) {
            self.state.tree_state.set_canonical_head(new_head.num_hash());
            self.persistence_state.finish(new_head.hash(), new_head.number());
            self.canonical_in_memory_state.set_canonical_head(new_head);
        } else {
            let num_hash = BlockNumHash { hash: backfill_hash, number: backfill_height };
            self.state.tree_state.set_canonical_head(num_hash);
            self.persistence_state.finish(backfill_hash, backfill_height);
        }

        // Remove executed blocks below the snap sync height
        let backfill_num_hash = self
            .provider
            .block_hash(backfill_height)?
            .map(|hash| BlockNumHash { hash, number: backfill_height })
            .unwrap_or(BlockNumHash { hash: backfill_hash, number: backfill_height });
        self.state.tree_state.remove_until(
            backfill_num_hash,
            self.persistence_state.last_persisted_block.hash,
            Some(backfill_num_hash),
        );

        self.metrics.engine.executed_blocks.set(self.state.tree_state.block_count() as f64);
        self.metrics.tree.canonical_chain_height.set(backfill_height as f64);

        // Trigger a pipeline run for MerkleExecute + Finish.
        // The orchestrator already set all other stage checkpoints to the snap target,
        // so only MerkleExecute (builds AccountsTrie/StoragesTrie from hashed leaves)
        // and Finish will run.
        self.emit_event(EngineApiEvent::BackfillAction(BackfillAction::Start(
            backfill_hash.into(),
        )));
        self.backfill_sync_state = BackfillSyncState::Pending;

        Ok(())
    }
}

//! Persistence scheduling and bookkeeping of the engine API tree handler.

use crate::{
    persistence::PersistenceResult,
    tree::{
        error::AdvancePersistenceError, payload_validator::EngineValidator, EngineApiTreeHandler,
        WaitForCaches, CHANGESET_CACHE_RETENTION_BLOCKS,
    },
};
use alloy_consensus::BlockHeader;
use alloy_eips::{BlockNumHash, NumHash};
use alloy_primitives::B256;
use reth_engine_primitives::{ConsensusEngineEvent, SlowBlockInfo};
use reth_errors::ProviderResult;
use reth_evm::ConfigureEvm;
use reth_payload_primitives::{BuiltPayload, PayloadTypes};
use reth_primitives_traits::NodePrimitives;
use reth_provider::{
    BalProvider, BlockReader, ChangeSetReader, DatabaseProviderFactory, ProviderError,
    PruneCheckpointReader, SaveBlocksInput, StageCheckpointReader, StateProviderFactory,
    StateReader, StorageChangeSetReader, StorageSettingsCache,
};
use reth_tasks::executor::{
    channel::{self, TryRecvError},
    time::Instant,
};
use std::time::Duration;
use tracing::debug;

impl<N, P, T, V, C> EngineApiTreeHandler<N, P, T, V, C>
where
    N: NodePrimitives,
    P: DatabaseProviderFactory
        + BlockReader<Block = N::Block, Header = N::BlockHeader>
        + StateProviderFactory
        + StateReader<Receipt = N::Receipt>
        + BalProvider
        + Clone
        + 'static,
    P::Provider: BlockReader<Block = N::Block, Header = N::BlockHeader>
        + PruneCheckpointReader
        + StageCheckpointReader
        + ChangeSetReader
        + StorageChangeSetReader
        + StorageSettingsCache
        + 'static,
    C: ConfigureEvm<Primitives = N> + 'static,
    T: PayloadTypes<BuiltPayload: BuiltPayload<Primitives = N>>,
    V: EngineValidator<T> + WaitForCaches,
{
    /// How many canonical blocks are retained in memory. A large count means persistence is
    /// falling behind execution.
    fn persistence_gap(&self) -> u64 {
        self.canonical_in_memory_state.canonical_chain().count() as u64
    }

    /// How many blocks beyond the configured in-memory buffer are awaiting persistence.
    fn persistence_backpressure_gap(&self) -> u64 {
        self.persistence_gap().saturating_sub(self.config.memory_block_buffer_target())
    }

    /// Returns `true` when the main loop should stop draining the tree input channel.
    ///
    /// This is the case when persistence is already running and the number of blocks beyond the
    /// configured in-memory buffer has reached the configured threshold.
    pub(super) fn should_backpressure(&self) -> bool {
        self.persistence_state.in_progress() &&
            self.persistence_backpressure_gap() >=
                self.config.persistence_backpressure_threshold()
    }

    /// Helper method to remove blocks and set the persistence state. This ensures we keep track of
    /// the current persistence action while we're removing blocks.
    pub(super) fn remove_blocks(&mut self, new_tip_num: u64) {
        debug!(target: "engine::tree", ?new_tip_num, last_persisted_block_number=?self.persistence_state.last_persisted_block.number, "Removing blocks using persistence task");
        if new_tip_num < self.persistence_state.last_persisted_block.number {
            debug!(target: "engine::tree", ?new_tip_num, "Starting remove blocks job");
            self.state.set_pending_sparse_trie_prune(false);
            let (tx, rx) = channel::bounded(1);
            let _ = self.persistence.remove_blocks_above(new_tip_num, tx);
            self.persistence_state.start_remove(new_tip_num, rx);
        }
    }

    /// Helper method to save blocks and set the persistence state. This ensures we keep track of
    /// the current persistence action while we're saving blocks.
    fn persist_blocks(&mut self, input: SaveBlocksInput<N>) {
        let highest_num_hash = input.last_block();
        debug!(target: "engine::tree", count=input.persist_rest_blocks().len(), blocks = ?input.persist_rest_blocks().iter().map(|block| block.recovered_block().num_hash()).collect::<Vec<_>>(), "Persisting blocks");

        let (tx, rx) = channel::bounded(1);
        let _ = self.persistence.save_blocks(input, tx);

        self.persistence_state.start_save(highest_num_hash, rx);
    }

    /// Triggers new persistence actions if no persistence task is currently in progress.
    ///
    /// This checks if we need to remove blocks (disk reorg) or save new blocks to disk.
    /// Persistence completion is handled separately via the `wait_for_event` method.
    pub(super) fn advance_persistence(&mut self) -> Result<(), AdvancePersistenceError> {
        if !self.persistence_state.in_progress() {
            let payload_build_active = self.payload_builds.is_active();
            if let Some(new_tip_num) = self.find_disk_reorg()? {
                self.remove_blocks(new_tip_num)
            } else if self.backfill_sync_state.is_pending_revalidation() &&
                !payload_build_active &&
                self.persistence_state.last_state_trie_persisted_block !=
                    self.persistence_state.last_persisted_block
            {
                let Some(input) = self.get_save_blocks_input(PersistTarget::Persisted) else {
                    return Err(AdvancePersistenceError::StateTrieCatchupUnavailable)
                };
                self.persist_blocks(input);
            } else if self.backfill_sync_state.is_pending_revalidation() && !payload_build_active {
                self.revalidate_pending_backfill()?;
            } else if let Some(input) = self.get_save_blocks_input(PersistTarget::Threshold) {
                self.persist_blocks(input);
            }
        }

        Ok(())
    }

    /// Persists all remaining blocks until none are left.
    pub(super) fn persist_until_complete(&mut self) -> Result<(), AdvancePersistenceError> {
        loop {
            // Wait for any in-progress persistence to complete (blocking)
            if let Some((rx, start_time, action)) = self.persistence_state.rx.take() {
                debug!(target: "engine::tree", ?action, "waiting for in-flight persistence");
                let result = rx.recv().map_err(|_| AdvancePersistenceError::ChannelClosed)?;
                self.on_persistence_complete(result, start_time)?;
                continue
            }

            // Persistence can finish against a branch that the in-memory canonical chain has
            // reorged away from. Unwind that stale disk branch before building a head-targeted
            // save.
            if let Some(new_tip_num) = self.find_disk_reorg()? {
                self.remove_blocks(new_tip_num);
                continue
            }

            let Some(input) = self.get_save_blocks_input(PersistTarget::Head) else {
                debug!(target: "engine::tree", "persistence complete, signaling termination");
                return Ok(())
            };

            debug!(target: "engine::tree", count = input.persist_rest_blocks().len(), "persisting remaining blocks before shutdown");
            self.persist_blocks(input);
        }
    }

    /// Tries to poll for a completed persistence task (non-blocking).
    ///
    /// Returns `true` if a persistence task was completed, `false` otherwise.
    pub(super) fn try_poll_persistence(&mut self) -> Result<bool, AdvancePersistenceError> {
        let Some((rx, start_time, action)) = self.persistence_state.rx.take() else {
            return Ok(false);
        };

        match rx.try_recv() {
            Ok(result) => {
                self.on_persistence_complete(result, start_time)?;
                Ok(true)
            }
            Err(TryRecvError::Empty) => {
                // Not ready yet, put it back
                self.persistence_state.rx = Some((rx, start_time, action));
                Ok(false)
            }
            Err(TryRecvError::Disconnected) => Err(AdvancePersistenceError::ChannelClosed),
        }
    }

    /// Handles a completed persistence task.
    pub(super) fn on_persistence_complete(
        &mut self,
        result: PersistenceResult,
        start_time: Instant,
    ) -> Result<(), AdvancePersistenceError> {
        self.metrics.engine.persistence_duration.record(start_time.elapsed());

        let PersistenceResult { last_block, last_state_trie_block, commit_duration } = result;
        debug_assert!(
            last_state_trie_block.number <= last_block.number,
            "state/trie frontier cannot exceed the last persisted block"
        );

        debug!(target: "engine::tree", ?last_block, ?last_state_trie_block, elapsed=?start_time.elapsed(), "Finished persisting, calling finish");
        self.persistence_state.finish(last_block, last_state_trie_block);

        let last_block_number = last_block.number;

        // Evict cached changesets for blocks below the eviction threshold.
        // Keep at least CHANGESET_CACHE_RETENTION_BLOCKS from the persisted tip, and also respect
        // the finalized block if set.
        let min_threshold = last_block_number.saturating_sub(CHANGESET_CACHE_RETENTION_BLOCKS);
        let eviction_threshold =
            if let Some(finalized) = self.canonical_in_memory_state.get_finalized_num_hash() {
                // Use the minimum of finalized block and retention threshold to be conservative
                finalized.number.min(min_threshold)
            } else {
                // When finalized is not set (e.g., on L2s), use the retention threshold
                min_threshold
            };
        debug!(
            target: "engine::tree",
            last_persisted = last_block_number,
            finalized_number = ?self.canonical_in_memory_state.get_finalized_num_hash().map(|f| f.number),
            eviction_threshold,
            "Evicting changesets below threshold"
        );
        self.state.tree_state.overlay_manager.evict_cached_changesets(eviction_threshold);

        self.on_new_persisted_block()?;

        self.purge_timing_stats(last_block_number, commit_duration);

        Ok(())
    }

    /// Removes timing stats for blocks at or below `below_number`.
    ///
    /// No-op when detailed block logging is disabled (no stats are recorded in that case).
    /// When `commit_duration` is provided and a slow block threshold is configured, checks
    /// each removed block against the threshold and emits a [`ConsensusEngineEvent::SlowBlock`]
    /// event for blocks that exceed it.
    pub(super) fn purge_timing_stats(
        &mut self,
        below_number: u64,
        commit_duration: Option<Duration>,
    ) {
        let threshold = self.config.slow_block_threshold();
        let check_slow = commit_duration.is_some() && threshold.is_some();

        // Two-pass: collect keys first because emit_event borrows &mut self.
        let keys_to_remove: Vec<B256> = self
            .execution_timing_stats
            .iter()
            .filter(|(_, stats)| stats.block_number <= below_number)
            .map(|(k, _)| *k)
            .collect();

        for key in keys_to_remove {
            let stats = self.execution_timing_stats.remove(&key).expect("key just found");
            if check_slow {
                let commit_dur = commit_duration.expect("checked above");
                // state_read_duration is already included in execution_duration
                let total_duration =
                    stats.execution_duration + stats.state_hash_duration + commit_dur;

                if total_duration > threshold.expect("checked above") {
                    self.emit_event(ConsensusEngineEvent::SlowBlock(SlowBlockInfo {
                        stats,
                        commit_duration: Some(commit_dur),
                        total_duration,
                    }));
                }
            }
        }
    }

    /// Returns the blocks and frontiers for the next persistence cycle, if one should start.
    ///
    /// Threshold persistence honors the normal scheduling gates and retains the configured
    /// in-memory block buffer. Persisted-target persistence catches the state/trie frontier up to
    /// the existing database tip. Head persistence bypasses those gates during shutdown and
    /// returns `None` once both persistence frontiers have reached the canonical head.
    pub(super) fn get_save_blocks_input(
        &self,
        target: PersistTarget,
    ) -> Option<SaveBlocksInput<N>> {
        // We will calculate the state root using the database, so we need to be sure there are no
        // changes
        debug_assert!(!self.persistence_state.in_progress());

        let prev_partial_state_trie = self.persistence_state.last_state_trie_persisted_block.number;
        let prev_db_tip = self.persistence_state.last_persisted_block.number;
        let canonical_head_number = self.state.tree_state.canonical_block_number();

        let (new_db_tip, new_partial_state_trie) = match target {
            PersistTarget::Head => (canonical_head_number, canonical_head_number),
            PersistTarget::Persisted => {
                // Catch-up persistence is the transition into pipeline sync, so it deliberately
                // runs while backfill is pending and bypasses the normal threshold gates.
                debug_assert!(self.backfill_sync_state.is_pending_revalidation());
                debug_assert!(!self.payload_builds.is_active());
                (prev_db_tip, prev_db_tip)
            }
            PersistTarget::Threshold => {
                if (self.config.suppress_persistence_during_build() &&
                    self.payload_builds.is_active()) ||
                    !self.backfill_sync_state.is_idle()
                {
                    return None
                }

                let persistence_threshold =
                    usize::try_from(self.config.persistence_threshold()).unwrap_or(usize::MAX);
                if self.canonical_in_memory_state.canonical_chain().count() <= persistence_threshold
                {
                    return None
                }

                let new_db_tip =
                    canonical_head_number.saturating_sub(self.config.memory_block_buffer_target());
                if new_db_tip <= prev_db_tip {
                    return None
                }

                let new_partial_state_trie = new_db_tip
                    .saturating_sub(self.config.num_state_masking_blocks())
                    .max(prev_partial_state_trie);
                (new_db_tip, new_partial_state_trie)
            }
        };

        debug_assert!(
            new_db_tip >= prev_db_tip,
            "disk reorg must be resolved before saving blocks"
        );
        debug_assert!(
            new_partial_state_trie >= prev_partial_state_trie,
            "disk reorg must be resolved before saving state/trie"
        );

        if new_db_tip == prev_db_tip && new_partial_state_trie == prev_partial_state_trie {
            return None
        }

        let mut blocks = Vec::new();
        let mut current_hash = self.state.tree_state.canonical_block_hash();

        debug!(
            target: "engine::tree",
            ?current_hash,
            ?prev_partial_state_trie,
            ?prev_db_tip,
            ?canonical_head_number,
            ?new_partial_state_trie,
            ?new_db_tip,
            target = ?target,
            "Returning save input"
        );
        while let Some(block) = self.state.tree_state.blocks_by_hash.get(&current_hash) {
            if block.recovered_block().number() <= prev_partial_state_trie {
                break;
            }

            if block.recovered_block().number() <= new_db_tip {
                blocks.push(block.clone());
            }

            current_hash = block.recovered_block().parent_hash();
        }

        // Reverse the order so that the oldest block comes first
        blocks.reverse();

        Some(SaveBlocksInput::new(
            blocks,
            prev_db_tip,
            prev_partial_state_trie,
            new_db_tip,
            new_partial_state_trie,
        ))
    }

    /// This clears the blocks from the in-memory tree state that have been persisted to the
    /// database.
    ///
    /// This also updates the canonical in-memory state to reflect the newest persisted block
    /// height.
    ///
    /// Assumes that `finish` has been called on the `persistence_state` at least once
    pub(super) fn on_new_persisted_block(&mut self) -> ProviderResult<()> {
        let in_memory_persisted_block = self.persistence_state.last_state_trie_persisted_block;

        // If we have an on-disk reorg, we need to handle it first before touching the in-memory
        // state.
        if let Some(remove_above) = self.find_disk_reorg()? {
            self.remove_blocks(remove_above);
            return Ok(())
        }

        let finalized = self.state.forkchoice_state_tracker.last_valid_finalized();
        // Trim the canonical in-memory state first: state providers build their overlays from the
        // canonical chain, so it must never reference blocks whose overlays the manager has
        // already pruned. `remove_before` does not read the canonical in-memory state, so the
        // order between the two trims is free to choose.
        self.canonical_in_memory_state.remove_persisted_blocks_until(
            self.persistence_state.last_persisted_block,
            in_memory_persisted_block.number,
        );
        self.remove_before(in_memory_persisted_block, finalized)?;
        // Persistence changes the overlay anchor. Prepare the remaining canonical range before
        // the next payload needs to read execution state against the new durable frontier.
        self.state.tree_state.overlay_manager.precompute_execution_overlay(
            self.state.tree_state.canonical_block_hash(),
            in_memory_persisted_block.hash,
        );
        self.state.set_pending_sparse_trie_prune(self.should_prune_sparse_trie());
        Ok(())
    }

    /// Returns whether sparse trie pruning should be attempted by the next sparse trie task.
    const fn should_prune_sparse_trie(&self) -> bool {
        self.config.use_state_root_task()
    }

    /// This method tries to detect whether on-disk and in-memory states have diverged. It might
    /// happen if a reorg is happening while we are persisting a block.
    fn find_disk_reorg(&self) -> ProviderResult<Option<u64>> {
        let mut canonical = self.state.tree_state.current_canonical_head;
        let mut persisted = self.persistence_state.last_persisted_block;

        let parent_num_hash = |num_hash: NumHash| -> ProviderResult<NumHash> {
            Ok(self
                .sealed_header_by_hash(num_hash.hash)?
                .ok_or(ProviderError::BlockHashNotFound(num_hash.hash))?
                .parent_num_hash())
        };

        // Happy path, canonical chain is ahead or equal to persisted chain.
        // Walk canonical chain back to make sure that it connects to persisted chain.
        while canonical.number > persisted.number {
            canonical = parent_num_hash(canonical)?;
        }

        // If we've reached persisted tip by walking the canonical chain back, everything is fine.
        if canonical == persisted {
            return Ok(None);
        }

        // At this point, we know that `persisted` block can't be reached by walking the canonical
        // chain back. In this case we need to truncate it to the first canonical block it connects
        // to.

        // Firstly, walk back until we reach the same height as `canonical`.
        while persisted.number > canonical.number {
            persisted = parent_num_hash(persisted)?;
        }

        debug_assert_eq!(persisted.number, canonical.number);

        // Now walk both chains back until we find a common ancestor.
        while persisted.hash != canonical.hash {
            canonical = parent_num_hash(canonical)?;
            persisted = parent_num_hash(persisted)?;
        }

        debug!(target: "engine::tree", remove_above=persisted.number, "on-disk reorg detected");

        Ok(Some(persisted.number))
    }

    /// Remove all blocks up to __and including__ the given block number.
    ///
    /// If a finalized hash is provided, the only non-canonical blocks which will be removed are
    /// those which have a fork point at or below the finalized hash.
    ///
    /// Canonical blocks below the upper bound will still be removed.
    pub(crate) fn remove_before(
        &mut self,
        upper_bound: BlockNumHash,
        finalized_hash: Option<B256>,
    ) -> ProviderResult<()> {
        // first fetch the finalized block number and then call the remove_before method on
        // tree_state
        let num = if let Some(hash) = finalized_hash {
            self.provider.block_number(hash)?.map(|number| BlockNumHash { number, hash })
        } else {
            None
        };

        self.state.tree_state.remove_until(
            upper_bound,
            self.persistence_state.last_persisted_block.hash,
            num,
        );
        self.metrics.engine.executed_blocks.set(self.state.tree_state.block_count() as f64);
        Ok(())
    }
}

/// Target for block persistence.
#[derive(Debug, Clone, Copy)]
pub(super) enum PersistTarget {
    /// Persist up to `canonical_head - memory_block_buffer_target`.
    Threshold,
    /// Persist all blocks up to and including the canonical head.
    Head,
    /// Persist state/trie updates through the persisted block frontier.
    Persisted,
}

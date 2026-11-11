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
use reth_chain_state::BlockState;
use reth_engine_primitives::{ConsensusEngineEvent, SlowBlockInfo};
use reth_errors::ProviderResult;
use reth_evm::ConfigureEvm;
use reth_payload_primitives::{BuiltPayload, PayloadTypes};
use reth_primitives_traits::{FastInstant as Instant, NodePrimitives};
use reth_provider::{
    BalProvider, BlockReader, ChangeSetReader, DatabaseProviderFactory, ProviderError,
    PruneCheckpointReader, SaveBlocksInput, StageCheckpointReader, StateProviderFactory,
    StateReader, StorageChangeSetReader, StorageSettingsCache,
};
use std::time::Duration;
use tokio::sync::oneshot;
use tracing::{debug, trace};

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
        self.state.tree_state.in_memory_state().block_counts().canonical as u64
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
            let (tx, rx) = crossbeam_channel::bounded(1);
            let _ = self.persistence.remove_blocks_above(new_tip_num, tx);
            self.persistence_state.start_remove(new_tip_num, rx);
        }
    }

    /// Helper method to save blocks and set the persistence state. This ensures we keep track of
    /// the current persistence action while we're saving blocks.
    fn persist_blocks(&mut self, input: SaveBlocksInput<N>) {
        let highest_num_hash = input.last_block();
        debug!(target: "engine::tree", count=input.persist_rest_blocks().len(), blocks = ?input.persist_rest_blocks().iter().map(|block| block.recovered_block().num_hash()).collect::<Vec<_>>(), "Persisting blocks");

        let (tx, rx) = crossbeam_channel::bounded(1);
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
                let Some(input) = self.get_save_blocks_input(PersistTarget::Persisted)? else {
                    return Err(AdvancePersistenceError::StateTrieCatchupUnavailable)
                };
                self.persist_blocks(input);
            } else if self.backfill_sync_state.is_pending_revalidation() && !payload_build_active {
                self.revalidate_pending_backfill()?;
            } else if let Some(input) = self.get_save_blocks_input(PersistTarget::Threshold)? {
                self.persist_blocks(input);
            }
        }

        Ok(())
    }

    /// Finishes termination by persisting all remaining blocks and signaling completion.
    ///
    /// This blocks until all persistence is complete or fails. Always signals completion,
    /// even if an error occurs.
    pub(super) fn finish_termination(
        &mut self,
        pending_termination: oneshot::Sender<()>,
    ) -> Result<(), AdvancePersistenceError> {
        trace!(target: "engine::tree", "finishing termination, persisting remaining blocks");
        let result = self.persist_until_complete();
        let _ = pending_termination.send(());
        result
    }

    /// Persists all remaining blocks until none are left.
    ///
    /// Returns an error, without persisting further blocks, if persistence fails or if the
    /// remaining blocks cannot be persisted.
    fn persist_until_complete(&mut self) -> Result<(), AdvancePersistenceError> {
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

            let Some(input) = self.get_save_blocks_input(PersistTarget::Head)? else {
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
            Err(crossbeam_channel::TryRecvError::Empty) => {
                // Not ready yet, put it back
                self.persistence_state.rx = Some((rx, start_time, action));
                Ok(false)
            }
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                Err(AdvancePersistenceError::ChannelClosed)
            }
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
        let finalized_number = self
            .state
            .tree_state
            .canonical_in_memory_state()
            .get_finalized_num_hash()
            .map(|finalized| finalized.number);
        let eviction_threshold = if let Some(finalized_number) = finalized_number {
            // Use the minimum of finalized block and retention threshold to be conservative.
            finalized_number.min(min_threshold)
        } else {
            // When finalized is not set (e.g., on L2s), use the retention threshold.
            min_threshold
        };
        debug!(
            target: "engine::tree",
            last_persisted = last_block_number,
            ?finalized_number,
            eviction_threshold,
            "Evicting changesets below threshold"
        );
        self.state.tree_state.overlay_manager().evict_cached_changesets(eviction_threshold);

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
    ///
    /// `Ok(None)` means that there is nothing to persist for `target`.
    ///
    /// # Errors
    ///
    /// [`AdvancePersistenceError::CanonicalBlocksNotInMemory`] if a cycle should start, but the
    /// canonical in-memory blocks are not the whole range between the persisted state/trie
    /// frontier and the new database tip. The tree state keeps that range in memory, so the
    /// engine cannot continue from this state.
    pub(super) fn get_save_blocks_input(
        &self,
        target: PersistTarget,
    ) -> Result<Option<SaveBlocksInput<N>>, AdvancePersistenceError> {
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
                    return Ok(None)
                }

                let persistence_threshold =
                    usize::try_from(self.config.persistence_threshold()).unwrap_or(usize::MAX);
                if self.state.tree_state.in_memory_state().block_counts().canonical <=
                    persistence_threshold
                {
                    return Ok(None)
                }

                let new_db_tip =
                    canonical_head_number.saturating_sub(self.config.memory_block_buffer_target());
                if new_db_tip <= prev_db_tip {
                    return Ok(None)
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
            return Ok(None)
        }

        let current_hash = self.state.tree_state.canonical_block_hash();

        // Walk the parent links of the canonical head's in-memory chain, under a single lookup.
        // Without a canonical head in memory there are no blocks.
        let head_state = self.state.tree_state.state_by_hash(current_hash);
        let mut blocks = head_state
            .iter()
            .flat_map(|head_state| head_state.chain())
            .take_while(|state| state.number() > prev_partial_state_trie)
            .filter(|state| state.number() <= new_db_tip)
            .map(BlockState::block)
            .collect::<Vec<_>>();

        // Reverse the order so that the oldest block comes first
        blocks.reverse();

        // The blocks must be the range `(prev_partial_state_trie, new_db_tip]` without a gap. They
        // are not if the canonical head is not in memory or if its in-memory chain does not reach
        // down to the state/trie frontier.
        let covers_range = new_db_tip.checked_sub(prev_partial_state_trie) ==
            Some(blocks.len() as u64) &&
            blocks
                .iter()
                .zip(prev_partial_state_trie + 1..)
                .all(|(block, number)| block.recovered_block().number() == number);
        if !covers_range {
            return Err(AdvancePersistenceError::CanonicalBlocksNotInMemory {
                canonical_head: current_hash,
                first_expected: prev_partial_state_trie + 1,
                last_expected: new_db_tip,
                found: blocks.len(),
                first_found: blocks.first().map(|block| block.recovered_block().num_hash()),
                last_found: blocks.last().map(|block| block.recovered_block().num_hash()),
            })
        }

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
        Ok(Some(SaveBlocksInput::new(
            blocks,
            prev_db_tip,
            prev_partial_state_trie,
            new_db_tip,
            new_partial_state_trie,
        )))
    }

    /// Records the newest persisted block and removes the blocks from the in-memory state that
    /// persistence made obsolete: the canonical blocks up to the persisted state/trie frontier and
    /// the non-canonical blocks that conflict with the finalized block.
    ///
    /// If the blocks on disk are not part of the canonical chain, this removes them from disk
    /// instead and leaves the in-memory state unchanged.
    ///
    /// Assumes that `finish` has been called on the `persistence_state` at least once.
    pub(super) fn on_new_persisted_block(&mut self) -> ProviderResult<()> {
        let in_memory_persisted_block = self.persistence_state.last_state_trie_persisted_block;

        // If we have an on-disk reorg, we need to handle it first before touching the in-memory
        // state.
        if let Some(remove_above) = self.find_disk_reorg()? {
            self.remove_blocks(remove_above);
            return Ok(())
        }

        // First fetch the finalized block number.
        let finalized =
            if let Some(hash) = self.state.forkchoice_state_tracker.last_valid_finalized() {
                self.provider.block_number(hash)?.map(|number| BlockNumHash { number, hash })
            } else {
                None
            };
        // Trims the canonical chain to the state/trie frontier and prunes the non-canonical blocks
        // that conflict with the finalized block.
        self.state.tree_state.on_blocks_persisted(
            self.persistence_state.last_persisted_block,
            in_memory_persisted_block,
            finalized,
        );
        // Persistence changes the overlay anchor. Prepare the remaining canonical range before
        // the next payload needs to read execution state against the new durable frontier.
        self.state.tree_state.overlay_manager().precompute_execution_overlay(
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
        let mut canonical = self.state.tree_state.canonical_head();
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

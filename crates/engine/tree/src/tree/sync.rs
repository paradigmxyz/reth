//! Backfill sync and block download handling of the engine API tree handler.

use crate::{
    backfill::{BackfillAction, BackfillSyncState},
    engine::{DownloadRequest, EngineApiEvent, FromEngine},
    tree::{
        error::{InsertBlockFatalError, InsertBlockProcessingError, InsertPayloadError},
        payload_validator::EngineValidator,
        BlockStatus, EngineApiTreeHandler, InsertPayloadOk, TreeAction, TreeEvent, WaitForCaches,
    },
};
use alloy_consensus::BlockHeader;
use alloy_eips::BlockNumHash;
use alloy_primitives::B256;
use alloy_rpc_types_engine::ForkchoiceState;
use reth_errors::ProviderResult;
use reth_evm::ConfigureEvm;
use reth_network_p2p::full_block::SealedBlockWithAccessList;
use reth_payload_primitives::{BuiltPayload, PayloadTypes};
use reth_primitives_traits::NodePrimitives;
use reth_provider::{
    BalProvider, BlockReader, ChangeSetReader, DatabaseProviderFactory, PruneCheckpointReader,
    StageCheckpointReader, StateProviderFactory, StateReader, StorageChangeSetReader,
    StorageSettingsCache,
};
use reth_stages_api::{ControlFlow, StageId};
use tracing::{debug, instrument, trace, warn};

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
    /// Invoked when previously requested blocks were downloaded.
    ///
    /// If the block count exceeds the configured batch size we're allowed to execute at once, this
    /// will execute the first batch and send the remaining blocks back through the channel so that
    /// block request processing isn't blocked for a long time.
    pub(super) fn on_downloaded(
        &mut self,
        mut blocks: Vec<SealedBlockWithAccessList<N::Block>>,
    ) -> Result<Option<TreeEvent>, InsertBlockFatalError> {
        if blocks.is_empty() {
            // nothing to execute
            return Ok(None)
        }

        trace!(target: "engine::tree", block_count = %blocks.len(), "received downloaded blocks");
        let batch = self.config.max_execute_block_batch_size().min(blocks.len());
        for block in blocks.drain(..batch) {
            if let Some(event) = self.on_downloaded_block(block)? {
                let needs_backfill = event.is_backfill_action();
                self.on_tree_event(event)?;
                if needs_backfill {
                    // can exit early if backfill is needed
                    return Ok(None)
                }
            }
        }

        // if we still have blocks to execute, send them as a followup request
        if !blocks.is_empty() {
            let _ = self.incoming_tx.send(FromEngine::DownloadedBlocks(blocks));
        }

        Ok(None)
    }

    /// Invoked if the backfill sync has finished to target.
    ///
    /// At this point we consider the block synced to the backfill target.
    ///
    /// Checks the tracked finalized block against the block on disk and requests another backfill
    /// run if the distance to the tip exceeds the threshold for another backfill run.
    ///
    /// This will also do the necessary housekeeping of the tree state, this includes:
    ///  - removing all blocks below the backfill height
    ///  - resetting the canonical in-memory state
    ///
    /// In case backfill resulted in an unwind, this will clear the tree state above the Finish
    /// checkpoint, and run backfill again if some stages are still below the unwind target.
    pub(super) fn on_backfill_sync_finished(
        &mut self,
        ctrl: ControlFlow,
    ) -> Result<(), InsertBlockFatalError> {
        debug!(target: "engine::tree", "received backfill sync finished event");
        self.backfill_sync_state = BackfillSyncState::Idle;

        // Pipeline unwound, memorize the invalid block.
        let backfill_height = if let ControlFlow::Unwind { bad_block, .. } = &ctrl {
            warn!(target: "engine::tree", invalid_block=?bad_block, "Bad block detected in unwind");
            // update the `invalid_headers` cache with the new invalid header
            self.state.invalid_headers.insert(**bad_block);

            // A stage error unwinds only the stages above the target, so the stages after the
            // failing one can still be below it. The Finish checkpoint is the highest block every
            // stage reached, and an unwind never leaves it above the target.
            Some(
                self.provider
                    .database_provider_ro()?
                    .get_stage_checkpoint(StageId::Finish)?
                    .unwrap_or_default()
                    .block_number,
            )
        } else {
            // backfill height is the block number that the backfill finished at
            ctrl.block_number()
        };

        // backfill height is the block number that the backfill finished at
        let Some(backfill_height) = backfill_height else { return Ok(()) };

        // state house keeping after backfill sync
        // remove all executed blocks below the backfill height
        //
        // We set the `finalized_num` to `Some(backfill_height)` to ensure we remove all state
        // before that
        let Some(backfill_num_hash) = self
            .provider
            .block_hash(backfill_height)?
            .map(|hash| BlockNumHash { hash, number: backfill_height })
        else {
            debug!(target: "engine::tree", ?ctrl, "Backfill block not found");
            return Ok(())
        };

        if ctrl.is_unwind() {
            // the node reset so we need to clear everything above that height so that backfill
            // height is the new canonical block.
            self.state.set_pending_sparse_trie_prune(false);
            self.state.tree_state.reset(backfill_num_hash)
        } else {
            self.state.tree_state.remove_until(
                backfill_num_hash,
                self.persistence_state.last_persisted_block.hash,
                Some(backfill_num_hash),
            );
        }

        self.metrics.engine.executed_blocks.set(self.state.tree_state.block_count() as f64);
        self.metrics.tree.canonical_chain_height.set(backfill_height as f64);

        // remove all buffered blocks below the backfill height
        self.state.buffer.remove_old_blocks(backfill_height);
        self.purge_timing_stats(backfill_height, None);
        // we remove all entries because now we're synced to the backfill target and consider this
        // the canonical chain
        self.canonical_in_memory_state.clear_state();

        if let Ok(Some(new_head)) = self.provider.sealed_header(backfill_height) {
            // update the tracked chain height, after backfill sync both the canonical height and
            // persisted height are the same
            self.state.tree_state.set_canonical_head(new_head.num_hash());
            self.persistence_state.finish(new_head.num_hash(), new_head.num_hash());

            // update the tracked canonical head
            self.canonical_in_memory_state.set_canonical_head(new_head);

            // If the pipeline reached the head of the syncing FCU, apply its safe and finalized
            // blocks now that the head is canonical. An unwind must wait for a new FCU because the
            // supplied sync target may have been invalidated.
            if !ctrl.is_unwind() {
                self.on_canonicalized_sync_target(backfill_num_hash.hash);
            }
        }

        // The database holds blocks up to the unwind target that not every stage has processed.
        // Run the lagging stages up to it before the tree executes or persists on top of them.
        if let ControlFlow::Unwind { target, .. } = &ctrl &&
            backfill_height < *target &&
            let Some(hash) = self.provider.block_hash(*target)?
        {
            self.dispatch_backfill_action(BackfillAction::Start(hash.into()));
            return Ok(())
        }

        // check if we need to run backfill again by comparing the most recent backfill target
        // height to the backfill height
        let Some(sync_target_state) = self.state.forkchoice_state_tracker.sync_target_state()
        else {
            return Ok(())
        };
        if !self.engine_kind.is_opstack() && sync_target_state.finalized_block_hash.is_zero() {
            // no finalized block, can't check distance on non-OP Stack chains
            return Ok(())
        }
        let target_hash = self.backfill_target_hash(sync_target_state);
        if target_hash.is_zero() {
            return Ok(())
        }
        // get the block number of the backfill target block, if we have it buffered
        let newest_target = self.state.buffer.block(&target_hash).map(|block| block.number());

        // The block number that the backfill finished at - if the progress or newest target is
        // None then we can't check the distance anyways.
        //
        // If both are Some, we perform another distance check and return the desired backfill
        // target
        if let Some(backfill_target) =
            ctrl.block_number().zip(newest_target).and_then(|(progress, target_number)| {
                // Determines whether or not we should run backfill again, in case
                // the new gap is still large enough and requires running backfill again
                self.backfill_sync_target(progress, target_number, None)
            })
        {
            // request another backfill run
            self.emit_event(EngineApiEvent::BackfillAction(BackfillAction::Start(
                backfill_target.into(),
            )));
            return Ok(())
        };

        // Check if there are more blocks to sync between current head and FCU target
        if let Some(lowest_buffered) =
            self.state.buffer.lowest_ancestor(&sync_target_state.head_block_hash)
        {
            let current_head_num = self.state.tree_state.current_canonical_head.number;
            let target_head_num = lowest_buffered.number();

            if let Some(distance) = self.distance_from_local_tip(current_head_num, target_head_num)
            {
                // There are blocks between current head and FCU target, download them
                debug!(
                    target: "engine::tree",
                    %current_head_num,
                    %target_head_num,
                    %distance,
                    "Backfill complete, downloading remaining blocks to reach FCU target"
                );

                self.emit_event(EngineApiEvent::Download(
                    DownloadRequest::block_range(lowest_buffered.parent_hash(), distance)
                        .with_access_lists(self.should_download_access_lists()),
                ));
                return Ok(());
            }
        } else {
            // We don't have the head block or any of its ancestors buffered. Request
            // a download for the head block which will then trigger further sync.
            debug!(
                target: "engine::tree",
                head_hash = %sync_target_state.head_block_hash,
                "Backfill complete but head block not buffered, requesting download"
            );
            self.emit_event(EngineApiEvent::Download(
                DownloadRequest::single_block(sync_target_state.head_block_hash)
                    .with_access_lists(self.should_download_access_lists()),
            ));
            return Ok(());
        }

        // try to close the gap by executing buffered blocks that are child blocks of the new head
        self.try_connect_buffered_blocks(self.state.tree_state.current_canonical_head)
    }

    /// Re-evaluates whether a deferred backfill is still required after persistence catches up.
    pub(super) fn revalidate_pending_backfill(&mut self) -> ProviderResult<()> {
        debug_assert!(self.backfill_sync_state.is_pending_revalidation());

        let sync_target_state = self.state.forkchoice_state_tracker.sync_target_state();
        let backfill_target = if let Some(state) = sync_target_state {
            let configured_target = self.backfill_target_hash(state);
            let target_hash =
                if configured_target.is_zero() { state.head_block_hash } else { configured_target };
            let target_number = if let Some(block) = self.state.buffer.block(&target_hash) {
                Some(block.number())
            } else {
                self.sealed_header_by_hash(target_hash)?.map(|header| header.number())
            };

            target_number.and_then(|target_number| {
                self.backfill_sync_target(
                    self.state.tree_state.canonical_block_number(),
                    target_number,
                    None,
                )
            })
        } else {
            None
        };

        if let Some(target) = backfill_target {
            self.dispatch_backfill_action(BackfillAction::Start(target.into()));
            return Ok(())
        }

        self.backfill_sync_state = BackfillSyncState::Idle;
        debug!(target: "engine::tree", "dropping deferred backfill after re-evaluation");

        // The target may have changed while persistence was draining. Resume the live-sync
        // download flow so a newer target can produce a fresh backfill decision.
        if let Some(state) = sync_target_state &&
            state.head_block_hash != self.state.tree_state.canonical_block_hash()
        {
            let target = self.lowest_buffered_ancestor_or(state.head_block_hash);
            self.send_event(EngineApiEvent::Download(DownloadRequest::single_block(target)));
        }

        Ok(())
    }

    /// Dispatches a validated backfill action to the orchestrator.
    pub(super) fn dispatch_backfill_action(&mut self, action: BackfillAction) {
        debug_assert!(
            self.backfill_sync_state.is_idle() ||
                self.backfill_sync_state.is_pending_revalidation(),
            "backfill action can only be dispatched while idle or pending revalidation"
        );
        self.backfill_sync_state = BackfillSyncState::Pending;
        self.metrics.engine.pipeline_runs.increment(1);
        // Forkchoice updates only forward finality to a running backfill, so the run starting here
        // gets the latest one up front.
        if let Some(state) = self.state.forkchoice_state_tracker.latest_state() {
            self.forward_finalized(state.finalized_block_hash);
        }
        debug!(target: "engine::tree", "emitting backfill action event");
        self.send_event(EngineApiEvent::BackfillAction(action));
    }

    // Forwards `finalized` to the backfill unless the forkchoice leaves it unset.
    pub(super) fn forward_finalized(&self, finalized: B256) {
        if !finalized.is_zero() {
            self.send_event(EngineApiEvent::BackfillAction(BackfillAction::UpdateFinalized(
                finalized,
            )));
        }
    }

    /// Returns whether block downloads should also attempt to fetch the blocks' access lists.
    ///
    /// This is the case once Amsterdam is active at the current canonical head, indicated by the
    /// head header carrying a block access list hash.
    ///
    /// This is a coarse gate: a download issued while the head is still pre-Amsterdam won't ask
    /// for access lists of blocks past the fork. Access list downloads are best-effort anyway, so
    /// those blocks simply take the regular execution path.
    pub(super) fn should_download_access_lists(&self) -> bool {
        self.canonical_in_memory_state.get_canonical_head().block_access_list_hash().is_some()
    }

    /// Returns true if the distance from the local tip to the block is greater than the configured
    /// threshold.
    ///
    /// If the `local_tip` is greater than the `block`, then this will return false.
    #[inline]
    pub(super) const fn exceeds_backfill_run_threshold(&self, local_tip: u64, block: u64) -> bool {
        block > local_tip && block - local_tip > self.config.backfill_run_threshold()
    }

    /// Returns how far the local tip is from the given block. If the local tip is at the same
    /// height or its block number is greater than the given block, this returns None.
    #[inline]
    const fn distance_from_local_tip(&self, local_tip: u64, block: u64) -> Option<u64> {
        if block > local_tip {
            Some(block - local_tip)
        } else {
            None
        }
    }

    /// Returns the block hash that backfill should target.
    ///
    /// Defaults to the finalized block hash. On OP Stack, the CL finalizes in large batches and the
    /// finalized hash can lag the canonical tip by a wide margin, so backfill targets the head.
    ///
    /// The zero-finalized optimistic-sync fallback for non-OP Stack chains is handled by
    /// [`Self::backfill_sync_target`].
    pub(super) const fn backfill_target_hash(&self, state: ForkchoiceState) -> B256 {
        if self.engine_kind.is_opstack() {
            state.head_block_hash
        } else {
            state.finalized_block_hash
        }
    }

    /// Returns the target hash to sync to if the distance from the local tip is greater than the
    /// threshold and we're not yet synced to the backfill target (see
    /// [`Self::backfill_target_hash`]).
    ///
    /// If this is invoked after a new block has been downloaded, the downloaded block could be
    /// the (missing) target block.
    pub(super) fn backfill_sync_target(
        &self,
        canonical_tip_num: u64,
        target_block_number: u64,
        downloaded_block: Option<BlockNumHash>,
    ) -> Option<B256> {
        let state = self.state.forkchoice_state_tracker.sync_target_state()?;
        let target_hash = self.backfill_target_hash(state);

        // check if the downloaded block is the tracked backfill target
        let exceeds_backfill_threshold = match downloaded_block.as_ref() {
            // if we downloaded the target block we can now check how far we're off
            Some(downloaded_block) if downloaded_block.hash == target_hash => {
                self.exceeds_backfill_run_threshold(canonical_tip_num, downloaded_block.number)
            }
            _ => match self.state.buffer.block(&target_hash) {
                // if we have buffered the target block, we should check how far we're off
                Some(buffered_target) => {
                    self.exceeds_backfill_run_threshold(canonical_tip_num, buffered_target.number())
                }
                // check if the distance exceeds the threshold for backfill sync
                None => self.exceeds_backfill_run_threshold(canonical_tip_num, target_block_number),
            },
        };

        if !exceeds_backfill_threshold {
            return None
        }

        // if we have already canonicalized the target block, we should skip backfill
        match self.provider.header_by_hash_or_number(target_hash.into()) {
            Err(err) => {
                warn!(target: "engine::tree", %err, "Failed to get backfill target block header");
                None
            }
            // we don't have the block yet and the distance exceeds the allowed threshold
            Ok(None) if !target_hash.is_zero() => Some(target_hash),
            Ok(None) => {
                // OPTIMISTIC SYNCING
                //
                // It can happen when the node is doing an
                // optimistic sync, where the CL has no knowledge of the finalized hash,
                // but is expecting the EL to sync as high
                // as possible before finalizing.
                //
                // This usually doesn't happen on ETH mainnet since CLs use the more
                // secure checkpoint syncing.
                //
                // However, optimism chains will do this. The risk of a reorg is however
                // low.
                debug!(target: "engine::tree", hash=?state.head_block_hash, "Setting head hash as an optimistic backfill target.");
                Some(state.head_block_hash)
            }
            // we're fully synced to the target block
            Ok(Some(_)) => None,
        }
    }

    /// This handles downloaded blocks that are shown to be disconnected from the canonical chain.
    ///
    /// This mainly compares the missing parent of the downloaded block with the current canonical
    /// tip, and decides whether or not backfill sync should be triggered.
    pub(super) fn on_disconnected_downloaded_block(
        &self,
        downloaded_block: BlockNumHash,
        missing_parent: BlockNumHash,
        head: BlockNumHash,
    ) -> Option<TreeEvent> {
        // compare the missing parent with the canonical tip
        if let Some(target) =
            self.backfill_sync_target(head.number, missing_parent.number, Some(downloaded_block))
        {
            trace!(target: "engine::tree", %target, "triggering backfill on downloaded block");
            return Some(TreeEvent::BackfillAction(BackfillAction::Start(target.into())));
        }

        // continue downloading the missing parent
        //
        // this happens if either:
        //  * the missing parent block num < canonical tip num
        //    * this case represents a missing block on a fork that is shorter than the canonical
        //      chain
        //  * the missing parent block num >= canonical tip num, but the number of missing blocks is
        //    less than the backfill threshold
        //    * this case represents a potentially long range of blocks to download and execute
        let request = if let Some(distance) =
            self.distance_from_local_tip(head.number, missing_parent.number)
        {
            trace!(target: "engine::tree", %distance, missing=?missing_parent, "downloading missing parent block range");
            DownloadRequest::block_range(missing_parent.hash, distance)
        } else {
            trace!(target: "engine::tree", missing=?missing_parent, "downloading missing parent block");
            // This happens when the missing parent is on an outdated
            // sidechain and we can only download the missing block itself
            DownloadRequest::single_block(missing_parent.hash)
        };

        Some(TreeEvent::Download(request.with_access_lists(self.should_download_access_lists())))
    }

    /// Handles a downloaded block that was successfully inserted as valid.
    ///
    /// If the block matches the sync target head, returns [`TreeAction::MakeCanonical`].
    /// If it matches a non-head sync target (safe or finalized), makes it canonical inline
    /// and triggers a download for the remaining blocks towards the actual head.
    /// Otherwise, tries to connect buffered blocks.
    pub(super) fn on_valid_downloaded_block(
        &mut self,
        block_num_hash: BlockNumHash,
    ) -> Result<Option<TreeEvent>, InsertBlockFatalError> {
        // check if we just inserted a block that's part of sync targets,
        // i.e. head, safe, or finalized
        if let Some(sync_target) = self.state.forkchoice_state_tracker.sync_target_state() &&
            sync_target.contains(block_num_hash.hash)
        {
            debug!(target: "engine::tree", ?sync_target, "appended downloaded sync target block");

            if sync_target.head_block_hash == block_num_hash.hash {
                // we just inserted the sync target head block, make it canonical
                return Ok(Some(TreeEvent::TreeAction(TreeAction::MakeCanonical {
                    sync_target_head: block_num_hash.hash,
                })))
            }

            // This block is part of the sync target (safe or finalized) but not the
            // head. Make it canonical and try to connect any buffered children, then
            // continue downloading towards the actual head if needed.
            self.make_canonical(block_num_hash.hash)?;
            self.try_connect_buffered_blocks(block_num_hash)?;

            // Check if we've reached the sync target head after connecting buffered
            // blocks (e.g. the head block may have already been buffered).
            if self.state.tree_state.canonical_block_hash() != sync_target.head_block_hash {
                let target = self.lowest_buffered_ancestor_or(sync_target.head_block_hash);
                trace!(target: "engine::tree", %target, "sync target head not yet reached, downloading head block");
                return Ok(Some(TreeEvent::Download(
                    DownloadRequest::single_block(target)
                        .with_access_lists(self.should_download_access_lists()),
                )))
            }

            return Ok(None)
        }
        trace!(target: "engine::tree", "appended downloaded block");
        self.try_connect_buffered_blocks(block_num_hash)?;
        Ok(None)
    }

    /// Invoked with a block downloaded from the network
    ///
    /// Returns an event with the appropriate action to take, such as:
    ///  - download more missing blocks
    ///  - try to canonicalize the target if the `block` is the tracked target (head) block.
    #[instrument(level = "debug", target = "engine::tree", skip_all, fields(block_hash = %block.hash(), block_num = %block.number()))]
    fn on_downloaded_block(
        &mut self,
        block: SealedBlockWithAccessList<N::Block>,
    ) -> Result<Option<TreeEvent>, InsertBlockFatalError> {
        let block_num_hash = block.num_hash();
        let lowest_buffered_ancestor = self.lowest_buffered_ancestor_or(block_num_hash.hash);
        if self.check_invalid_ancestor_with_head(lowest_buffered_ancestor, &block)?.is_some() {
            return Ok(None)
        }

        if !self.backfill_sync_state.is_idle() {
            return Ok(None)
        }

        // try to append the block
        match self.insert_block(block) {
            Ok(InsertPayloadOk::Inserted(BlockStatus::Valid)) => {
                return self.on_valid_downloaded_block(block_num_hash);
            }
            Ok(InsertPayloadOk::Inserted(BlockStatus::Disconnected { head, missing_ancestor })) => {
                // block is not connected to the canonical head, we need to download
                // its missing branch first
                return Ok(self.on_disconnected_downloaded_block(
                    block_num_hash,
                    missing_ancestor,
                    head,
                ))
            }
            Ok(InsertPayloadOk::AlreadySeen(_)) => {
                trace!(target: "engine::tree", "downloaded block already executed");
            }
            Err(err) => {
                if let InsertPayloadError::Block(err) = err {
                    debug!(target: "engine::tree", err=%err.kind(), "failed to insert downloaded block");
                    if let Err(InsertBlockProcessingError::Fatal(fatal)) =
                        self.on_insert_block_error(err)
                    {
                        warn!(target: "engine::tree", %fatal, "fatal error occurred while inserting downloaded block");
                    }
                }
            }
        }
        Ok(None)
    }
}

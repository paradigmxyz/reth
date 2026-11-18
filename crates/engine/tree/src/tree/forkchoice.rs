//! Forkchoice update handling of the engine API tree handler.

use crate::{
    backfill::BackfillAction,
    engine::{DownloadRequest, EngineApiEvent},
    tree::{
        payload_validator::EngineValidator, EngineApiTreeHandler, TreeEvent, TreeOutcome,
        WaitForCaches,
    },
};
use alloy_consensus::BlockHeader;
use alloy_primitives::B256;
use alloy_rpc_types_engine::{ForkchoiceState, PayloadStatus, PayloadStatusEnum};
use reth_chain_state::NewCanonicalChain;
use reth_engine_primitives::OnForkChoiceUpdated;
use reth_errors::ProviderResult;
use reth_evm::ConfigureEvm;
use reth_payload_builder::{BuildNewPayload, PayloadBuilderLease};
use reth_payload_primitives::{BuiltPayload, PayloadAttributes, PayloadTypes};
use reth_primitives_traits::{NodePrimitives, SealedHeader};
use reth_provider::{
    BalProvider, BlockReader, ChangeSetReader, DatabaseProviderFactory, ProviderError,
    PruneCheckpointReader, StageCheckpointReader, StateProviderFactory, StateReader,
    StorageChangeSetReader, StorageSettingsCache,
};
use tracing::{debug, error, instrument, trace, warn};

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
    /// Invoked when we receive a new forkchoice update message. Calls into the blockchain tree
    /// to resolve chain forks and ensure that the Execution Layer is working with the latest valid
    /// chain.
    ///
    /// These responses should adhere to the [Engine API Spec for
    /// `engine_forkchoiceUpdated`](https://github.com/ethereum/execution-apis/blob/main/src/engine/paris.md#specification-1).
    ///
    /// Returns an error if an internal error occurred like a database error.
    #[instrument(level = "debug", target = "engine::tree", skip_all, fields(head = % state.head_block_hash, safe = % state.safe_block_hash,finalized = % state.finalized_block_hash))]
    pub(super) fn on_forkchoice_updated(
        &mut self,
        state: ForkchoiceState,
        attrs: Option<T::PayloadAttributes>,
    ) -> ProviderResult<TreeOutcome<OnForkChoiceUpdated>> {
        trace!(target: "engine::tree", ?attrs, "invoked forkchoice update");

        // Record metrics
        self.record_forkchoice_metrics();

        // Pre-validation of forkchoice state
        if let Some(early_result) = self.validate_forkchoice_state(state)? {
            return Ok(TreeOutcome::new(early_result));
        }

        // Return early if we are on the correct fork
        if let Some(result) = self.handle_canonical_head(state, &attrs)? {
            return Ok(result);
        }

        // Attempt to apply a chain update when the head differs from our canonical chain.
        // This handles reorgs and chain extensions by making the specified head canonical.
        if let Some(result) = self.apply_chain_update(state, &attrs)? {
            return Ok(result);
        }

        // Fallback that ensures to catch up to the network's state.
        self.handle_missing_block(state)
    }

    /// Records metrics for forkchoice updated calls
    fn record_forkchoice_metrics(&self) {
        self.canonical_in_memory_state.on_forkchoice_update_received();
    }

    /// Pre-validates the forkchoice state and returns early if validation fails.
    ///
    /// Returns `Some(OnForkChoiceUpdated)` if validation fails and an early response should be
    /// returned. Returns `None` if validation passes and processing should continue.
    pub(super) fn validate_forkchoice_state(
        &mut self,
        state: ForkchoiceState,
    ) -> ProviderResult<Option<OnForkChoiceUpdated>> {
        if state.head_block_hash.is_zero() {
            return Ok(Some(OnForkChoiceUpdated::invalid_state()));
        }

        // Check if the new head hash is connected to any ancestor that we previously marked as
        // invalid
        let lowest_buffered_ancestor_fcu = self.lowest_buffered_ancestor_or(state.head_block_hash);
        if let Some(status) = self.check_invalid_ancestor(lowest_buffered_ancestor_fcu)? {
            return Ok(Some(OnForkChoiceUpdated::with_invalid(status)));
        }

        if !self.backfill_sync_state.is_idle() {
            // Forward the head and finalized block, since a long-running backfill such as snap
            // must follow them. A run awaiting revalidation has not started and reads the latest
            // forkchoice when it does.
            if self.backfill_sync_state.is_pending() || self.backfill_sync_state.is_active() {
                self.send_event(EngineApiEvent::BackfillAction(BackfillAction::UpdateTarget(
                    state.head_block_hash,
                )));
                self.forward_finalized(state.finalized_block_hash);
            }
            // We can only process new forkchoice updates if the pipeline is idle, since it requires
            // exclusive access to the database
            trace!(target: "engine::tree", "Pipeline is syncing, skipping forkchoice update");
            return Ok(Some(OnForkChoiceUpdated::syncing()));
        }

        Ok(None)
    }

    /// Handles the case where the forkchoice head is already canonical.
    ///
    /// Returns `Some(TreeOutcome<OnForkChoiceUpdated>)` if the head is already canonical and
    /// processing is complete. Returns `None` if the head is not canonical and processing
    /// should continue.
    pub(super) fn handle_canonical_head(
        &mut self,
        state: ForkchoiceState,
        attrs: &Option<T::PayloadAttributes>, // Changed to reference
    ) -> ProviderResult<Option<TreeOutcome<OnForkChoiceUpdated>>> {
        // Process the forkchoice update by trying to make the head block canonical
        //
        // We can only process this forkchoice update if:
        // - we have the `head` block
        // - the head block is part of a chain that is connected to the canonical chain. This
        //   includes reorgs.
        //
        // Performing a FCU involves:
        // - marking the FCU's head block as canonical
        // - updating in memory state to reflect the new canonical chain
        // - updating canonical state trackers
        // - emitting a canonicalization event for the new chain (including reorg)
        // - if we have payload attributes, delegate them to the payload service

        if self.state.tree_state.canonical_block_hash() != state.head_block_hash {
            return Ok(None);
        }

        trace!(target: "engine::tree", "fcu head hash is already canonical");

        if !self.is_consistent_forkchoice_state(state, None)? {
            return Ok(Some(TreeOutcome::new(OnForkChoiceUpdated::invalid_state())));
        }

        // Update the safe and finalized blocks and ensure their values are valid
        if let Err(outcome) = self.ensure_consistent_forkchoice_state(state) {
            // safe or finalized hashes are invalid
            return Ok(Some(TreeOutcome::new(outcome)));
        }

        self.payload_validator.on_canonical_head_changed(state.head_block_hash, &self.state);

        // Process payload attributes if the head is already canonical
        if let Some(attr) = attrs {
            let tip = self
                .sealed_header_by_hash(self.state.tree_state.canonical_block_hash())?
                .ok_or_else(|| {
                    // If we can't find the canonical block, then something is wrong and we need
                    // to return an error
                    ProviderError::HeaderNotFound(state.head_block_hash.into())
                })?;
            // Clone only when we actually need to process the attributes
            let updated = self.process_payload_attributes(attr.clone(), &tip, state);
            return Ok(Some(TreeOutcome::new(updated)));
        }

        // The head block is already canonical
        Ok(Some(Self::valid_outcome(state)))
    }

    /// Applies chain update for the new head block and processes payload attributes.
    ///
    /// This method handles the case where the forkchoice head differs from our current canonical
    /// head. It attempts to make the specified head block canonical by:
    /// - Checking if the head is already part of the canonical chain
    /// - Applying chain reorganizations (reorgs) if necessary
    /// - Processing payload attributes if provided
    /// - Returning the appropriate forkchoice update response
    ///
    /// Returns `Some(TreeOutcome<OnForkChoiceUpdated>)` if a chain update was successfully applied.
    /// Returns `None` if no chain update was needed or possible.
    pub(super) fn apply_chain_update(
        &mut self,
        state: ForkchoiceState,
        attrs: &Option<T::PayloadAttributes>,
    ) -> ProviderResult<Option<TreeOutcome<OnForkChoiceUpdated>>> {
        // Check if the head is already part of the canonical chain
        if let Ok(Some(canonical_header)) = self.find_canonical_header(state.head_block_hash) {
            debug!(target: "engine::tree", head = canonical_header.number(), "fcu head block is already canonical");

            // For OpStack, or if explicitly configured, the proposers are allowed to reorg their
            // own chain at will, so we need to always trigger a new payload job if requested.
            let always_trigger_payload_job = self.engine_kind.is_opstack() ||
                self.config.always_process_payload_attributes_on_canonical_head();

            // A canonical ancestor below the latest known finalized block can never become the
            // head again, because this would reorg out the finalized block. Such a forkchoice
            // update exceeds the supported reorg depth and is rejected regardless of the payload
            // attributes:
            // <https://github.com/ethereum/execution-apis/blob/bf20b4083284e677db19e7f3871bd669b88354a6/src/engine/paris.md?plain=1#L221>
            //
            // The stored finalized block is used because a forkchoice update MAY carry a zero
            // finalized hash without clearing previously established finality.
            if !always_trigger_payload_job &&
                self.canonical_in_memory_state
                    .get_finalized_num_hash()
                    .is_some_and(|finalized| canonical_header.number() < finalized.number)
            {
                debug!(target: "engine::tree", head = canonical_header.number(), "rejecting canonical ancestor fcu below the finalized block");
                return Ok(Some(TreeOutcome::new(OnForkChoiceUpdated::too_deep_reorg())));
            }

            if !self.is_consistent_forkchoice_state(state, None)? {
                return Ok(Some(TreeOutcome::new(OnForkChoiceUpdated::invalid_state())));
            }

            // We need to effectively unwind the _canonical_ chain to the FCU's head, which is
            // part of the canonical chain. We need to update the latest block state to reflect
            // the canonical ancestor. This ensures that state providers and the transaction
            // pool operate with the correct chain state after forkchoice update processing, and
            // new payloads built on the reorg'd head will be added to the tree immediately.
            if always_trigger_payload_job && self.config.unwind_canonical_header() {
                self.update_latest_block_to_canonical_ancestor(&canonical_header)?;
            }

            // A canonical ancestor at or above the latest known finalized block can become the
            // parent of the next block, e.g. when the CL wants to reorg out the current head.
            // The canonical chain remains untouched here; the block built on the ancestor
            // triggers the actual reorg once it is inserted via newPayload and FCU'd.
            if let Some(attr) = attrs {
                debug!(target: "engine::tree", head = canonical_header.number(), "handling payload attributes for canonical head");
                // Clone only when we actually need to process the attributes
                let updated =
                    self.process_payload_attributes(attr.clone(), &canonical_header, state);
                return Ok(Some(TreeOutcome::new(updated)));
            }

            // The head block is already canonical and we're not processing payload attributes,
            // so we're not triggering a payload job and can return right away
            return Ok(Some(Self::valid_outcome(state)));
        }

        // Ensure we can apply a new chain update for the head block
        if let Some(chain_update) = self.on_new_head(state.head_block_hash)? {
            if !self.is_consistent_forkchoice_state(state, Some(&chain_update))? {
                return Ok(Some(TreeOutcome::new(OnForkChoiceUpdated::invalid_state())));
            }

            let tip = chain_update.tip().clone_sealed_header();
            self.on_canonical_chain_update(chain_update);

            // Update the safe and finalized blocks and ensure their values are valid
            if let Err(outcome) = self.ensure_consistent_forkchoice_state(state) {
                // safe or finalized hashes are invalid
                return Ok(Some(TreeOutcome::new(outcome)));
            }

            if let Some(attr) = attrs {
                // Clone only when we actually need to process the attributes
                let updated = self.process_payload_attributes(attr.clone(), &tip, state);
                return Ok(Some(TreeOutcome::new(updated)));
            }

            return Ok(Some(Self::valid_outcome(state)));
        }

        Ok(None)
    }

    /// Handles the case where the head block is missing and needs to be downloaded.
    ///
    /// This is the fallback case when all other forkchoice update scenarios have been exhausted.
    /// Returns a `TreeOutcome` with syncing status and download event.
    pub(super) fn handle_missing_block(
        &self,
        state: ForkchoiceState,
    ) -> ProviderResult<TreeOutcome<OnForkChoiceUpdated>> {
        // We don't have the block to perform the forkchoice update
        // We assume the FCU is valid and at least the head is missing,
        // so we need to start syncing to it
        //
        // find the appropriate target to sync to, if we don't have the safe block hash then we
        // start syncing to the safe block via backfill first
        let target = if self.state.forkchoice_state_tracker.is_empty() &&
        // check that safe block is valid and missing
        !state.safe_block_hash.is_zero() &&
        self.find_canonical_header(state.safe_block_hash).ok().flatten().is_none()
        {
            debug!(target: "engine::tree", "missing safe block on initial FCU, downloading safe block");
            state.safe_block_hash
        } else {
            state.head_block_hash
        };

        let target = self.lowest_buffered_ancestor_or(target);
        trace!(target: "engine::tree", %target, "downloading missing block");

        Ok(TreeOutcome::new(OnForkChoiceUpdated::valid(PayloadStatus::from_status(
            PayloadStatusEnum::Syncing,
        )))
        .with_event(TreeEvent::Download(
            DownloadRequest::single_block(target)
                .with_access_lists(self.should_download_access_lists()),
        )))
    }

    /// Attempts to find the header for the given block hash if it is canonical.
    ///
    /// A persisted header can still be found by hash while its disk reorg is pending, so it is
    /// only canonical if it is not above the canonical head and matches the canonical hash at its
    /// height. The in-memory canonical hash is preferred over the persisted one.
    pub fn find_canonical_header(
        &self,
        hash: B256,
    ) -> Result<Option<SealedHeader<N::BlockHeader>>, ProviderError> {
        if let Some(header) = self.canonical_in_memory_state.header_by_hash(hash) {
            return Ok(Some(header))
        }

        let Some(header) = self.provider.sealed_header_by_hash(hash)? else { return Ok(None) };
        // Reorged-out headers remain on disk until persistence catches up, so reject old tips above
        // the in-memory head and use in-memory hashes to reject old blocks at shared heights.
        let number = header.number();
        if number > self.canonical_in_memory_state.get_canonical_block_number() {
            return Ok(None)
        }
        let canonical_hash =
            if let Some(hash) = self.canonical_in_memory_state.hash_by_number(number) {
                Some(hash)
            } else {
                self.provider.block_hash(number)?
            };

        Ok((canonical_hash == Some(hash)).then_some(header))
    }

    /// Checks that nonzero safe and finalized hashes belong to the chain defined by the FCU head.
    ///
    /// The [Engine API forkchoiceUpdated specification] requires `-38002` when a `VALID` head's
    /// safe or finalized hash is outside that chain, and requires all forkchoice state updates to
    /// be atomic. In direct FCU processing, this check must therefore run before canonicalizing
    /// the head or updating either marker; checking only after canonicalization would leave an
    /// invalid reorg applied.
    ///
    /// `chain_update` describes a proposed commit or reorg that has not yet been applied. Its new
    /// blocks and the canonical prefix below its first block form the proposed chain. Without a
    /// chain update, the proposed head is already canonical, so only canonical blocks through its
    /// height are eligible. Canonical-prefix hashes are resolved via
    /// [`Self::find_canonical_header`], which rejects stale persisted headers whose disk reorg
    /// cleanup is pending. A zero safe or finalized hash leaves that marker unchanged.
    /// Returns `Ok(false)` for an unknown or off-chain hash and propagates provider errors.
    ///
    /// [Engine API forkchoiceUpdated specification]: https://github.com/ethereum/execution-apis/blob/main/src/engine/paris.md#specification-1
    pub(super) fn is_consistent_forkchoice_state(
        &self,
        state: ForkchoiceState,
        chain_update: Option<&NewCanonicalChain<N>>,
    ) -> ProviderResult<bool> {
        let canonical_head_number = match chain_update {
            Some(chain_update) => {
                let new = chain_update.new_blocks();
                // Only the canonical prefix below the new branch remains on the proposed chain.
                new.first().expect("non empty chain").block_number() - 1
            }
            None => {
                let Some(head) = self.find_canonical_header(state.head_block_hash)? else {
                    return Ok(false)
                };
                head.number()
            }
        };

        for hash in [state.finalized_block_hash, state.safe_block_hash] {
            if hash.is_zero() || chain_update.is_some_and(|update| update.contains(hash)) {
                continue
            }
            let Some(header) = self.find_canonical_header(hash)? else { return Ok(false) };
            if header.number() > canonical_head_number {
                return Ok(false)
            }
        }
        Ok(true)
    }

    /// Updates the tracked finalized block if we have it.
    fn update_finalized_block(
        &self,
        finalized_block_hash: B256,
    ) -> Result<(), OnForkChoiceUpdated> {
        if finalized_block_hash.is_zero() {
            return Ok(())
        }

        match self.find_canonical_header(finalized_block_hash) {
            Ok(None) => {
                debug!(target: "engine::tree", "Finalized block not found in canonical chain");
                // if the finalized block is not known, we can't update the finalized block
                return Err(OnForkChoiceUpdated::invalid_state())
            }
            Ok(Some(finalized)) => {
                if Some(finalized.num_hash()) !=
                    self.canonical_in_memory_state.get_finalized_num_hash()
                {
                    // we're also persisting the finalized block on disk so we can reload it on
                    // restart this is required by optimism which queries the finalized block: <https://github.com/ethereum-optimism/optimism/blob/c383eb880f307caa3ca41010ec10f30f08396b2e/op-node/rollup/sync/start.go#L65-L65>
                    let _ = self.persistence.save_finalized_block_number(finalized.number());
                    self.canonical_in_memory_state.set_finalized(finalized.clone());
                    // Update finalized block height metric
                    self.metrics.tree.finalized_block_height.set(finalized.number() as f64);
                }
            }
            Err(err) => {
                error!(target: "engine::tree", %err, "Failed to fetch finalized block header");
            }
        }

        Ok(())
    }

    /// Updates the tracked safe block if we have it
    fn update_safe_block(&self, safe_block_hash: B256) -> Result<(), OnForkChoiceUpdated> {
        if safe_block_hash.is_zero() {
            return Ok(())
        }

        match self.find_canonical_header(safe_block_hash) {
            Ok(None) => {
                debug!(target: "engine::tree", "Safe block not found in canonical chain");
                // if the safe block is not known, we can't update the safe block
                return Err(OnForkChoiceUpdated::invalid_state())
            }
            Ok(Some(safe)) => {
                if Some(safe.num_hash()) != self.canonical_in_memory_state.get_safe_num_hash() {
                    // we're also persisting the safe block on disk so we can reload it on
                    // restart this is required by optimism which queries the safe block: <https://github.com/ethereum-optimism/optimism/blob/c383eb880f307caa3ca41010ec10f30f08396b2e/op-node/rollup/sync/start.go#L65-L65>
                    let _ = self.persistence.save_safe_block_number(safe.number());
                    self.canonical_in_memory_state.set_safe(safe.clone());
                    // Update safe block height metric
                    self.metrics.tree.safe_block_height.set(safe.number() as f64);
                }
            }
            Err(err) => {
                error!(target: "engine::tree", %err, "Failed to fetch safe block header");
            }
        }

        Ok(())
    }

    /// Ensures that the given forkchoice state is consistent, assuming the head block has been
    /// made canonical.
    ///
    /// If the forkchoice state is consistent, this will return Ok(()). Otherwise, this will
    /// return an instance of [`OnForkChoiceUpdated`] that is INVALID.
    ///
    /// This also updates the safe and finalized blocks in the [`CanonicalInMemoryState`], if they
    /// are consistent with the head block.
    ///
    /// [`CanonicalInMemoryState`]: reth_chain_state::CanonicalInMemoryState
    pub(super) fn ensure_consistent_forkchoice_state(
        &self,
        state: ForkchoiceState,
    ) -> Result<(), OnForkChoiceUpdated> {
        // Ensure that the finalized block, if not zero, is known and in the canonical chain
        // after the head block is canonicalized.
        //
        // This ensures that the finalized block is consistent with the head block, i.e. the
        // finalized block is an ancestor of the head block.
        self.update_finalized_block(state.finalized_block_hash)?;

        // Also ensure that the safe block, if not zero, is known and in the canonical chain
        // after the head block is canonicalized.
        //
        // This ensures that the safe block is consistent with the head block, i.e. the safe
        // block is an ancestor of the head block.
        self.update_safe_block(state.safe_block_hash)
    }

    /// Validates the payload attributes with respect to the header and fork choice state.
    ///
    /// This is called during `engine_forkchoiceUpdated` when the CL provides payload attributes,
    /// indicating it wants the EL to start building a new block.
    ///
    /// Runs [`PayloadValidator::validate_payload_attributes_against_header`](reth_engine_primitives::PayloadValidator::validate_payload_attributes_against_header) to ensure
    /// `payloadAttributes.timestamp > headBlock.timestamp` per the Engine API spec.
    ///
    /// If validation passes, sends the attributes to the payload builder to start a new
    /// payload job. If it fails, returns `INVALID_PAYLOAD_ATTRIBUTES` without rolling back
    /// the forkchoice update.
    ///
    /// Note: At this point, the fork choice update is considered to be VALID, however, we can still
    /// return an error if the payload attributes are invalid.
    pub(super) fn process_payload_attributes(
        &mut self,
        attributes: T::PayloadAttributes,
        head: &N::BlockHeader,
        state: ForkchoiceState,
    ) -> OnForkChoiceUpdated {
        if let Err(err) =
            self.payload_validator.validate_payload_attributes_against_header(&attributes, head)
        {
            warn!(target: "engine::tree", %err, ?head, "Invalid payload attributes");
            return OnForkChoiceUpdated::invalid_payload_attributes()
        }

        // 8. Client software MUST begin a payload build process building on top of
        //    forkchoiceState.headBlockHash and identified via buildProcessId value if
        //    payloadAttributes is not null and the forkchoice state has been updated successfully.
        //    The build process is specified in the Payload building section.

        // Acquire this before preparing resources because state-root setup can already start
        // workers that need the current in-memory overlay.
        let payload_build = self.payload_builds.acquire();

        let resources = self
            .payload_validator
            .payload_builder_resources(
                state.head_block_hash,
                head,
                attributes.timestamp(),
                &mut self.state,
            )
            .with_lease(PayloadBuilderLease::new(payload_build));

        // send the payload to the builder and return the receiver for the pending payload
        // id, initiating payload job is handled asynchronously
        let pending_payload_id = self.payload_builder.send_new_payload(BuildNewPayload {
            parent_hash: state.head_block_hash,
            attributes,
            resources,
        });

        // Client software MUST respond to this method call in the following way:
        // {
        //      payloadStatus: {
        //          status: VALID,
        //          latestValidHash: forkchoiceState.headBlockHash,
        //          validationError: null
        //      },
        //      payloadId: buildProcessId
        // }
        //
        // if the payload is deemed VALID and the build process has begun.
        OnForkChoiceUpdated::updated_with_pending_payload_id(
            PayloadStatus::new(PayloadStatusEnum::Valid, Some(state.head_block_hash)),
            pending_payload_id,
        )
    }
}

use futures::{stream::BoxStream, Future, StreamExt};
use itertools::Either;
use reth_blockchain_tree_api::{
    error::{BlockchainTreeError, CanonicalError, InsertBlockError, InsertBlockErrorKind},
    BlockStatus, BlockValidationKind, BlockchainTreeEngine, CanonicalOutcome, InsertPayloadOk,
};
use reth_db_api::database::Database;
use reth_engine_primitives::EngineTypes;
use reth_errors::{BlockValidationError, ProviderResult, RethError, RethResult};
use reth_network_p2p::{
    sync::{NetworkSyncUpdater, SyncState},
    BlockClient,
};
use reth_payload_builder::PayloadBuilderHandle;
use reth_payload_primitives::{PayloadAttributes, PayloadBuilderAttributes};
use reth_payload_validator::ExecutionPayloadValidator;
use reth_primitives::{
    constants::EPOCH_SLOTS, BlockNumHash, BlockNumber, Head, Header, SealedBlock, SealedHeader,
    B256,
};
use reth_provider::{
    BlockIdReader, BlockReader, BlockSource, CanonChainTracker, ChainSpecProvider, ProviderError,
    StageCheckpointReader,
};
use reth_rpc_types::engine::{
    CancunPayloadFields, ExecutionPayload, ForkchoiceState, PayloadStatus, PayloadStatusEnum,
    PayloadValidationError,
};
use reth_stages_api::{ControlFlow, Pipeline, PipelineTarget, StageId};
use reth_tasks::TaskSpawner;
use reth_tokio_util::EventSender;
use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::{Duration, Instant},
};
use tokio::sync::{
    mpsc::{self, UnboundedSender},
    oneshot,
};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::*;

mod message;
pub use message::{BeaconEngineMessage, OnForkChoiceUpdated};

mod error;
pub use error::{
    BeaconConsensusEngineError, BeaconEngineResult, BeaconForkChoiceUpdateError,
    BeaconOnNewPayloadError,
};

mod invalid_headers;
pub use invalid_headers::InvalidHeaderCache;

mod event;
pub use event::{BeaconConsensusEngineEvent, ConsensusEngineLiveSyncProgress};

mod handle;
pub use handle::BeaconConsensusEngineHandle;

mod forkchoice;
pub use forkchoice::{ForkchoiceStateHash, ForkchoiceStateTracker, ForkchoiceStatus};

mod metrics;
use metrics::EngineMetrics;

pub mod sync;
use sync::{EngineSyncController, EngineSyncEvent};

/// Hooks for running during the main loop of
/// [consensus engine][`crate::engine::BeaconConsensusEngine`].
pub mod hooks;
use hooks::{EngineHookContext, EngineHookEvent, EngineHooks, EngineHooksController, PolledHook};

#[cfg(test)]
pub mod test_utils;

/// The maximum number of invalid headers that can be tracked by the engine.
const MAX_INVALID_HEADERS: u32 = 512u32;

/// The largest gap for which the tree will be used for sync. See docs for `pipeline_run_threshold`
/// for more information.
///
/// This is the default threshold, the distance to the head that the tree will be used for sync.
/// If the distance exceeds this threshold, the pipeline will be used for sync.
pub const MIN_BLOCKS_FOR_PIPELINE_RUN: u64 = EPOCH_SLOTS;

/// Represents a pending forkchoice update.
///
/// This type encapsulates the necessary components for a pending forkchoice update
/// in the context of a beacon consensus engine.
///
/// It consists of:
/// - The current fork choice state.
/// - Optional payload attributes specific to the engine type.
/// - Sender for the result of an oneshot channel, conveying the outcome of the fork choice update.
type PendingForkchoiceUpdate<PayloadAttributes> =
    (ForkchoiceState, Option<PayloadAttributes>, oneshot::Sender<RethResult<OnForkChoiceUpdated>>);

/// The beacon consensus engine is the driver that switches between historical and live sync.
///
/// The beacon consensus engine is itself driven by messages from the Consensus Layer, which are
/// received by Engine API (JSON-RPC).
///
/// The consensus engine is idle until it receives the first
/// [`BeaconEngineMessage::ForkchoiceUpdated`] message from the CL which would initiate the sync. At
/// first, the consensus engine would run the [Pipeline] until the latest known block hash.
/// Afterward, it would attempt to create/restore the [`BlockchainTreeEngine`] from the blocks
/// that are currently available. In case the restoration is successful, the consensus engine would
/// run in a live sync mode, populating the [`BlockchainTreeEngine`] with new blocks as they arrive
/// via engine API and downloading any missing blocks from the network to fill potential gaps.
///
/// The consensus engine has two data input sources:
///
/// ## New Payload (`engine_newPayloadV{}`)
///
/// The engine receives new payloads from the CL. If the payload is connected to the canonical
/// chain, it will be fully validated added to a chain in the [`BlockchainTreeEngine`]: `VALID`
///
/// If the payload's chain is disconnected (at least 1 block is missing) then it will be buffered:
/// `SYNCING` ([`BlockStatus::Disconnected`]).
///
/// ## Forkchoice Update (FCU) (`engine_forkchoiceUpdatedV{}`)
///
/// This contains the latest forkchoice state and the payload attributes. The engine will attempt to
/// make a new canonical chain based on the `head_hash` of the update and trigger payload building
/// if the `payload_attrs` are present and the FCU is `VALID`.
///
/// The `head_hash` forms a chain by walking backwards from the `head_hash` towards the canonical
/// blocks of the chain.
///
/// Making a new canonical chain can result in the following relevant outcomes:
///
/// ### The chain is connected
///
/// All blocks of the `head_hash`'s chain are present in the [`BlockchainTreeEngine`] and are
/// committed to the canonical chain. This also includes reorgs.
///
/// ### The chain is disconnected
///
/// In this case the [`BlockchainTreeEngine`] doesn't know how the new chain connects to the
/// existing canonical chain. It could be a simple commit (new blocks extend the current head) or a
/// re-org that requires unwinding the canonical chain.
///
/// This further distinguishes between two variants:
///
/// #### `head_hash`'s block exists
///
/// The `head_hash`'s block was already received/downloaded, but at least one block is missing to
/// form a _connected_ chain. The engine will attempt to download the missing blocks from the
/// network by walking backwards (`parent_hash`), and then try to make the block canonical as soon
/// as the chain becomes connected.
///
/// However, it still can be the case that the chain and the FCU is `INVALID`.
///
/// #### `head_hash` block is missing
///
/// This is similar to the previous case, but the `head_hash`'s block is missing. At which point the
/// engine doesn't know where the new head will point to: new chain could be a re-org or a simple
/// commit. The engine will download the missing head first and then proceed as in the previous
/// case.
///
/// # Panics
///
/// If the future is polled more than once. Leads to undefined state.
#[must_use = "Future does nothing unless polled"]
#[allow(missing_debug_implementations)]
pub struct BeaconConsensusEngine<DB, BT, Client, EngineT>
where
    DB: Database,
    Client: BlockClient,
    BT: BlockchainTreeEngine
        + BlockReader
        + BlockIdReader
        + CanonChainTracker
        + StageCheckpointReader,
    EngineT: EngineTypes,
{
    /// Controls syncing triggered by engine updates.
    sync: EngineSyncController<DB, Client>,
    /// The type we can use to query both the database and the blockchain tree.
    blockchain: BT,
    /// Used for emitting updates about whether the engine is syncing or not.
    sync_state_updater: Box<dyn NetworkSyncUpdater>,
    /// The Engine API message receiver.
    engine_message_stream: BoxStream<'static, BeaconEngineMessage<EngineT>>,
    /// A clone of the handle
    handle: BeaconConsensusEngineHandle<EngineT>,
    /// Tracks the received forkchoice state updates received by the CL.
    forkchoice_state_tracker: ForkchoiceStateTracker,
    /// The payload store.
    payload_builder: PayloadBuilderHandle<EngineT>,
    /// Validator for execution payloads
    payload_validator: ExecutionPayloadValidator,
    /// Current blockchain tree action.
    blockchain_tree_action: Option<BlockchainTreeAction<EngineT>>,
    /// Pending forkchoice update.
    /// It is recorded if we cannot process the forkchoice update because
    /// a hook with database read-write access is active.
    /// This is a temporary solution to always process missed FCUs.
    pending_forkchoice_update: Option<PendingForkchoiceUpdate<EngineT::PayloadAttributes>>,
    /// Tracks the header of invalid payloads that were rejected by the engine because they're
    /// invalid.
    invalid_headers: InvalidHeaderCache,
    /// After downloading a block corresponding to a recent forkchoice update, the engine will
    /// check whether or not we can connect the block to the current canonical chain. If we can't,
    /// we need to download and execute the missing parents of that block.
    ///
    /// When the block can't be connected, its block number will be compared to the canonical head,
    /// resulting in a heuristic for the number of missing blocks, or the size of the gap between
    /// the new block and the canonical head.
    ///
    /// If the gap is larger than this threshold, the engine will download and execute the missing
    /// blocks using the pipeline. Otherwise, the engine, sync controller, and blockchain tree will
    /// be used to download and execute the missing blocks.
    pipeline_run_threshold: u64,
    hooks: EngineHooksController,
    /// Sender for engine events.
    event_sender: EventSender<BeaconConsensusEngineEvent>,
    /// Consensus engine metrics.
    metrics: EngineMetrics,
}

impl<DB, BT, Client, EngineT> BeaconConsensusEngine<DB, BT, Client, EngineT>
where
    DB: Database + Unpin + 'static,
    BT: BlockchainTreeEngine
        + BlockReader
        + BlockIdReader
        + CanonChainTracker
        + StageCheckpointReader
        + ChainSpecProvider
        + 'static,
    Client: BlockClient + 'static,
    EngineT: EngineTypes + Unpin,
{
    /// Create a new instance of the [`BeaconConsensusEngine`].
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        client: Client,
        pipeline: Pipeline<DB>,
        blockchain: BT,
        task_spawner: Box<dyn TaskSpawner>,
        sync_state_updater: Box<dyn NetworkSyncUpdater>,
        max_block: Option<BlockNumber>,
        payload_builder: PayloadBuilderHandle<EngineT>,
        target: Option<B256>,
        pipeline_run_threshold: u64,
        hooks: EngineHooks,
    ) -> RethResult<(Self, BeaconConsensusEngineHandle<EngineT>)> {
        let (to_engine, rx) = mpsc::unbounded_channel();
        Self::with_channel(
            client,
            pipeline,
            blockchain,
            task_spawner,
            sync_state_updater,
            max_block,
            payload_builder,
            target,
            pipeline_run_threshold,
            to_engine,
            Box::pin(UnboundedReceiverStream::from(rx)),
            hooks,
        )
    }

    /// Create a new instance of the [`BeaconConsensusEngine`] using the given channel to configure
    /// the [`BeaconEngineMessage`] communication channel.
    ///
    /// By default the engine is started with idle pipeline.
    /// The pipeline can be launched immediately in one of the following ways descending in
    /// priority:
    /// - Explicit [`Option::Some`] target block hash provided via a constructor argument.
    /// - The process was previously interrupted amidst the pipeline run. This is checked by
    ///   comparing the checkpoints of the first ([`StageId::Headers`]) and last
    ///   ([`StageId::Finish`]) stages. In this case, the latest available header in the database is
    ///   used as the target.
    ///
    /// Propagates any database related error.
    #[allow(clippy::too_many_arguments)]
    pub fn with_channel(
        client: Client,
        pipeline: Pipeline<DB>,
        blockchain: BT,
        task_spawner: Box<dyn TaskSpawner>,
        sync_state_updater: Box<dyn NetworkSyncUpdater>,
        max_block: Option<BlockNumber>,
        payload_builder: PayloadBuilderHandle<EngineT>,
        target: Option<B256>,
        pipeline_run_threshold: u64,
        to_engine: UnboundedSender<BeaconEngineMessage<EngineT>>,
        engine_message_stream: BoxStream<'static, BeaconEngineMessage<EngineT>>,
        hooks: EngineHooks,
    ) -> RethResult<(Self, BeaconConsensusEngineHandle<EngineT>)> {
        let event_sender = EventSender::default();
        let handle = BeaconConsensusEngineHandle::new(to_engine, event_sender.clone());
        let sync = EngineSyncController::new(
            pipeline,
            client,
            task_spawner.clone(),
            max_block,
            blockchain.chain_spec(),
            event_sender.clone(),
        );
        let mut this = Self {
            sync,
            payload_validator: ExecutionPayloadValidator::new(blockchain.chain_spec()),
            blockchain,
            sync_state_updater,
            engine_message_stream,
            handle: handle.clone(),
            forkchoice_state_tracker: Default::default(),
            payload_builder,
            invalid_headers: InvalidHeaderCache::new(MAX_INVALID_HEADERS),
            blockchain_tree_action: None,
            pending_forkchoice_update: None,
            pipeline_run_threshold,
            hooks: EngineHooksController::new(hooks),
            event_sender,
            metrics: EngineMetrics::default(),
        };

        let maybe_pipeline_target = match target {
            // Provided target always takes precedence.
            target @ Some(_) => target,
            None => this.check_pipeline_consistency()?,
        };

        if let Some(target) = maybe_pipeline_target {
            this.sync.set_pipeline_sync_target(target.into());
        }

        Ok((this, handle))
    }

    /// Returns current [`EngineHookContext`] that's used for polling engine hooks.
    fn current_engine_hook_context(&self) -> RethResult<EngineHookContext> {
        Ok(EngineHookContext {
            tip_block_number: self.blockchain.canonical_tip().number,
            finalized_block_number: self
                .blockchain
                .finalized_block_number()
                .map_err(RethError::Provider)?,
        })
    }

    /// Set the next blockchain tree action.
    fn set_blockchain_tree_action(&mut self, action: BlockchainTreeAction<EngineT>) {
        let previous_action = self.blockchain_tree_action.replace(action);
        debug_assert!(previous_action.is_none(), "Pre-existing action found");
    }

    /// Pre-validate forkchoice update and check whether it can be processed.
    ///
    /// This method returns the update outcome if validation fails or
    /// the node is syncing and the update cannot be processed at the moment.
    fn pre_validate_forkchoice_update(
        &mut self,
        state: ForkchoiceState,
    ) -> ProviderResult<Option<OnForkChoiceUpdated>> {
        if state.head_block_hash.is_zero() {
            return Ok(Some(OnForkChoiceUpdated::invalid_state()))
        }

        // check if the new head hash is connected to any ancestor that we previously marked as
        // invalid
        let lowest_buffered_ancestor_fcu = self.lowest_buffered_ancestor_or(state.head_block_hash);
        if let Some(status) = self.check_invalid_ancestor(lowest_buffered_ancestor_fcu)? {
            return Ok(Some(OnForkChoiceUpdated::with_invalid(status)))
        }

        if self.sync.is_pipeline_active() {
            // We can only process new forkchoice updates if the pipeline is idle, since it requires
            // exclusive access to the database
            trace!(target: "consensus::engine", "Pipeline is syncing, skipping forkchoice update");
            return Ok(Some(OnForkChoiceUpdated::syncing()))
        }

        Ok(None)
    }

    /// Process the result of attempting to make forkchoice state head hash canonical.
    ///
    /// # Returns
    ///
    /// A forkchoice state update outcome or fatal error.
    fn on_forkchoice_updated_make_canonical_result(
        &mut self,
        state: ForkchoiceState,
        mut attrs: Option<EngineT::PayloadAttributes>,
        make_canonical_result: Result<CanonicalOutcome, CanonicalError>,
        elapsed: Duration,
    ) -> Result<OnForkChoiceUpdated, CanonicalError> {
        match make_canonical_result {
            Ok(outcome) => {
                let should_update_head = match &outcome {
                    CanonicalOutcome::AlreadyCanonical { head, header } => {
                        self.on_head_already_canonical(head, header, &mut attrs)
                    }
                    CanonicalOutcome::Committed { head } => {
                        // new VALID update that moved the canonical chain forward
                        debug!(target: "consensus::engine", hash=?state.head_block_hash, number=head.number, "Canonicalized new head");
                        true
                    }
                };

                if should_update_head {
                    let head = outcome.header();
                    let _ = self.update_head(head.clone());
                    self.event_sender.notify(BeaconConsensusEngineEvent::CanonicalChainCommitted(
                        Box::new(head.clone()),
                        elapsed,
                    ));
                }

                // Validate that the forkchoice state is consistent.
                let on_updated = if let Some(invalid_fcu_response) =
                    self.ensure_consistent_forkchoice_state(state)?
                {
                    trace!(target: "consensus::engine", ?state, "Forkchoice state is inconsistent");
                    invalid_fcu_response
                } else if let Some(attrs) = attrs {
                    // the CL requested to build a new payload on top of this new VALID head
                    let head = outcome.into_header().unseal();
                    self.process_payload_attributes(attrs, head, state)
                } else {
                    OnForkChoiceUpdated::valid(PayloadStatus::new(
                        PayloadStatusEnum::Valid,
                        Some(state.head_block_hash),
                    ))
                };
                Ok(on_updated)
            }
            Err(err) => {
                if err.is_fatal() {
                    error!(target: "consensus::engine", %err, "Encountered fatal error");
                    Err(err)
                } else {
                    Ok(OnForkChoiceUpdated::valid(
                        self.on_failed_canonical_forkchoice_update(&state, err)?,
                    ))
                }
            }
        }
    }

    /// Invoked when head hash references a `VALID` block that is already canonical.
    ///
    /// Returns `true` if the head needs to be updated.
    fn on_head_already_canonical(
        &self,
        head: &BlockNumHash,
        header: &SealedHeader,
        attrs: &mut Option<EngineT::PayloadAttributes>,
    ) -> bool {
        // On Optimism, the proposers are allowed to reorg their own chain at will.
        #[cfg(feature = "optimism")]
        if self.blockchain.chain_spec().is_optimism() {
            debug!(
                target: "consensus::engine",
                fcu_head_num=?header.number,
                current_head_num=?head.number,
                "[Optimism] Allowing beacon reorg to old head"
            );
            return true
        }

        // 2. Client software MAY skip an update of the forkchoice state and MUST NOT begin a
        //    payload build process if `forkchoiceState.headBlockHash` references a `VALID` ancestor
        //    of the head of canonical chain, i.e. the ancestor passed payload validation process
        //    and deemed `VALID`. In the case of such an event, client software MUST return
        //    `{payloadStatus: {status: VALID, latestValidHash: forkchoiceState.headBlockHash,
        //    validationError: null}, payloadId: null}`
        if head != &header.num_hash() {
            attrs.take();
        }

        debug!(
            target: "consensus::engine",
            fcu_head_num=?header.number,
            current_head_num=?head.number,
            "Ignoring beacon update to old head"
        );
        false
    }

    /// Invoked when we receive a new forkchoice update message. Calls into the blockchain tree
    /// to resolve chain forks and ensure that the Execution Layer is working with the latest valid
    /// chain.
    ///
    /// These responses should adhere to the [Engine API Spec for
    /// `engine_forkchoiceUpdated`](https://github.com/ethereum/execution-apis/blob/main/src/engine/paris.md#specification-1).
    ///
    /// Returns an error if an internal error occurred like a database error.
    fn on_forkchoice_updated(
        &mut self,
        state: ForkchoiceState,
        attrs: Option<EngineT::PayloadAttributes>,
        tx: oneshot::Sender<RethResult<OnForkChoiceUpdated>>,
    ) {
        self.metrics.forkchoice_updated_messages.increment(1);
        self.blockchain.on_forkchoice_update_received(&state);
        trace!(target: "consensus::engine", ?state, "Received new forkchoice state update");

        match self.pre_validate_forkchoice_update(state) {
            Ok(on_updated_result) => {
                if let Some(on_updated) = on_updated_result {
                    // Pre-validate forkchoice state update and return if it's invalid
                    // or cannot be processed at the moment.
                    self.on_forkchoice_updated_status(state, on_updated, tx);
                } else if let Some(hook) = self.hooks.active_db_write_hook() {
                    // We can only process new forkchoice updates if no hook with db write is
                    // running, since it requires exclusive access to the
                    // database
                    let replaced_pending =
                        self.pending_forkchoice_update.replace((state, attrs, tx));
                    warn!(
                        target: "consensus::engine",
                        hook = %hook.name(),
                        head_block_hash = ?state.head_block_hash,
                        safe_block_hash = ?state.safe_block_hash,
                        finalized_block_hash = ?state.finalized_block_hash,
                        replaced_pending = ?replaced_pending.map(|(state, _, _)| state),
                        "Hook is in progress, delaying forkchoice update. \
                        This may affect the performance of your node as a validator."
                    );
                } else {
                    self.set_blockchain_tree_action(
                        BlockchainTreeAction::MakeForkchoiceHeadCanonical { state, attrs, tx },
                    );
                }
            }
            Err(error) => {
                let _ = tx.send(Err(error.into()));
            }
        }
    }

    /// Called after the forkchoice update status has been resolved.
    /// Depending on the outcome, the method updates the sync state and notifies the listeners
    /// about new processed FCU.
    fn on_forkchoice_updated_status(
        &mut self,
        state: ForkchoiceState,
        on_updated: OnForkChoiceUpdated,
        tx: oneshot::Sender<RethResult<OnForkChoiceUpdated>>,
    ) {
        // send the response to the CL ASAP
        let status = on_updated.forkchoice_status();
        let _ = tx.send(Ok(on_updated));

        // update the forkchoice state tracker
        self.forkchoice_state_tracker.set_latest(state, status);

        match status {
            ForkchoiceStatus::Invalid => {}
            ForkchoiceStatus::Valid => {
                // FCU head is valid, we're no longer syncing
                self.sync_state_updater.update_sync_state(SyncState::Idle);
                // node's fully synced, clear active download requests
                self.sync.clear_block_download_requests();
            }
            ForkchoiceStatus::Syncing => {
                // we're syncing
                self.sync_state_updater.update_sync_state(SyncState::Syncing);
            }
        }

        // notify listeners about new processed FCU
        self.event_sender.notify(BeaconConsensusEngineEvent::ForkchoiceUpdated(state, status));
    }

    /// Check if the pipeline is consistent (all stages have the checkpoint block numbers no less
    /// than the checkpoint of the first stage).
    ///
    /// This will return the pipeline target if:
    ///  * the pipeline was interrupted during its previous run
    ///  * a new stage was added
    ///  * stage data was dropped manually through `reth stage drop ...`
    ///
    /// # Returns
    ///
    /// A target block hash if the pipeline is inconsistent, otherwise `None`.
    fn check_pipeline_consistency(&self) -> RethResult<Option<B256>> {
        // If no target was provided, check if the stages are congruent - check if the
        // checkpoint of the last stage matches the checkpoint of the first.
        let first_stage_checkpoint = self
            .blockchain
            .get_stage_checkpoint(*StageId::ALL.first().unwrap())?
            .unwrap_or_default()
            .block_number;

        // Skip the first stage as we've already retrieved it and comparing all other checkpoints
        // against it.
        for stage_id in StageId::ALL.iter().skip(1) {
            let stage_checkpoint =
                self.blockchain.get_stage_checkpoint(*stage_id)?.unwrap_or_default().block_number;

            // If the checkpoint of any stage is less than the checkpoint of the first stage,
            // retrieve and return the block hash of the latest header and use it as the target.
            if stage_checkpoint < first_stage_checkpoint {
                debug!(
                    target: "consensus::engine",
                    first_stage_checkpoint,
                    inconsistent_stage_id = %stage_id,
                    inconsistent_stage_checkpoint = stage_checkpoint,
                    "Pipeline sync progress is inconsistent"
                );
                return Ok(self.blockchain.block_hash(first_stage_checkpoint)?)
            }
        }

        Ok(None)
    }

    /// Returns a new [`BeaconConsensusEngineHandle`] that can be cloned and shared.
    ///
    /// The [`BeaconConsensusEngineHandle`] can be used to interact with this
    /// [`BeaconConsensusEngine`]
    pub fn handle(&self) -> BeaconConsensusEngineHandle<EngineT> {
        self.handle.clone()
    }

    /// Returns true if the distance from the local tip to the block is greater than the configured
    /// threshold.
    ///
    /// If the `local_tip` is greater than the `block`, then this will return false.
    #[inline]
    const fn exceeds_pipeline_run_threshold(&self, local_tip: u64, block: u64) -> bool {
        block > local_tip && block - local_tip > self.pipeline_run_threshold
    }

    /// Returns the finalized hash to sync to if the distance from the local tip to the block is
    /// greater than the configured threshold and we're not synced to the finalized block yet
    /// yet (if we've seen that block already).
    ///
    /// If this is invoked after a new block has been downloaded, the downloaded block could be the
    /// (missing) finalized block.
    fn can_pipeline_sync_to_finalized(
        &self,
        canonical_tip_num: u64,
        target_block_number: u64,
        downloaded_block: Option<BlockNumHash>,
    ) -> Option<B256> {
        let sync_target_state = self.forkchoice_state_tracker.sync_target_state();

        // check if the distance exceeds the threshold for pipeline sync
        let mut exceeds_pipeline_run_threshold =
            self.exceeds_pipeline_run_threshold(canonical_tip_num, target_block_number);

        // check if the downloaded block is the tracked finalized block
        if let Some(ref buffered_finalized) = sync_target_state
            .as_ref()
            .and_then(|state| self.blockchain.buffered_header_by_hash(state.finalized_block_hash))
        {
            // if we have buffered the finalized block, we should check how far
            // we're off
            exceeds_pipeline_run_threshold =
                self.exceeds_pipeline_run_threshold(canonical_tip_num, buffered_finalized.number);
        }

        // If this is invoked after we downloaded a block we can check if this block is the
        // finalized block
        if let (Some(downloaded_block), Some(ref state)) = (downloaded_block, sync_target_state) {
            if downloaded_block.hash == state.finalized_block_hash {
                // we downloaded the finalized block
                exceeds_pipeline_run_threshold =
                    self.exceeds_pipeline_run_threshold(canonical_tip_num, downloaded_block.number);
            }
        }

        // if the number of missing blocks is greater than the max, run the
        // pipeline
        if exceeds_pipeline_run_threshold {
            if let Some(state) = sync_target_state {
                // if we have already canonicalized the finalized block, we should
                // skip the pipeline run
                match self.blockchain.header_by_hash_or_number(state.finalized_block_hash.into()) {
                    Err(err) => {
                        warn!(target: "consensus::engine", %err, "Failed to get finalized block header");
                    }
                    Ok(None) => {
                        // ensure the finalized block is known (not the zero hash)
                        if !state.finalized_block_hash.is_zero() {
                            // we don't have the block yet and the distance exceeds the allowed
                            // threshold
                            return Some(state.finalized_block_hash)
                        }

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
                        debug!(target: "consensus::engine", hash=?state.head_block_hash, "Setting head hash as an optimistic pipeline target.");
                        return Some(state.head_block_hash)
                    }
                    Ok(Some(_)) => {
                        // we're fully synced to the finalized block
                        // but we want to continue downloading the missing parent
                    }
                }
            }
        }

        None
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

    /// If validation fails, the response MUST contain the latest valid hash:
    ///
    ///   - The block hash of the ancestor of the invalid payload satisfying the following two
    ///     conditions:
    ///     - It is fully validated and deemed VALID
    ///     - Any other ancestor of the invalid payload with a higher blockNumber is INVALID
    ///   - 0x0000000000000000000000000000000000000000000000000000000000000000 if the above
    ///     conditions are satisfied by a `PoW` block.
    ///   - null if client software cannot determine the ancestor of the invalid payload satisfying
    ///     the above conditions.
    fn latest_valid_hash_for_invalid_payload(
        &mut self,
        parent_hash: B256,
    ) -> ProviderResult<Option<B256>> {
        // Check if parent exists in side chain or in canonical chain.
        if self.blockchain.find_block_by_hash(parent_hash, BlockSource::Any)?.is_some() {
            return Ok(Some(parent_hash))
        }

        // iterate over ancestors in the invalid cache
        // until we encounter the first valid ancestor
        let mut current_hash = parent_hash;
        let mut current_header = self.invalid_headers.get(&current_hash);
        while let Some(header) = current_header {
            current_hash = header.parent_hash;
            current_header = self.invalid_headers.get(&current_hash);

            // If current_header is None, then the current_hash does not have an invalid
            // ancestor in the cache, check its presence in blockchain tree
            if current_header.is_none() &&
                self.blockchain.find_block_by_hash(current_hash, BlockSource::Any)?.is_some()
            {
                return Ok(Some(current_hash))
            }
        }
        Ok(None)
    }

    /// Prepares the invalid payload response for the given hash, checking the
    /// database for the parent hash and populating the payload status with the latest valid hash
    /// according to the engine api spec.
    fn prepare_invalid_response(&mut self, mut parent_hash: B256) -> ProviderResult<PayloadStatus> {
        // Edge case: the `latestValid` field is the zero hash if the parent block is the terminal
        // PoW block, which we need to identify by looking at the parent's block difficulty
        if let Ok(Some(parent)) = self.blockchain.header_by_hash_or_number(parent_hash.into()) {
            if !parent.is_zero_difficulty() {
                parent_hash = B256::ZERO;
            }
        }

        let valid_parent_hash = self.latest_valid_hash_for_invalid_payload(parent_hash)?;
        Ok(PayloadStatus::from_status(PayloadStatusEnum::Invalid {
            validation_error: PayloadValidationError::LinksToRejectedPayload.to_string(),
        })
        .with_latest_valid_hash(valid_parent_hash.unwrap_or_default()))
    }

    /// Checks if the given `check` hash points to an invalid header, inserting the given `head`
    /// block into the invalid header cache if the `check` hash has a known invalid ancestor.
    ///
    /// Returns a payload status response according to the engine API spec if the block is known to
    /// be invalid.
    fn check_invalid_ancestor_with_head(
        &mut self,
        check: B256,
        head: B256,
    ) -> ProviderResult<Option<PayloadStatus>> {
        // check if the check hash was previously marked as invalid
        let Some(header) = self.invalid_headers.get(&check) else { return Ok(None) };

        // populate the latest valid hash field
        let status = self.prepare_invalid_response(header.parent_hash)?;

        // insert the head block into the invalid header cache
        self.invalid_headers.insert_with_invalid_ancestor(head, header);

        Ok(Some(status))
    }

    /// Checks if the given `head` points to an invalid header, which requires a specific response
    /// to a forkchoice update.
    fn check_invalid_ancestor(&mut self, head: B256) -> ProviderResult<Option<PayloadStatus>> {
        // check if the head was previously marked as invalid
        let Some(header) = self.invalid_headers.get(&head) else { return Ok(None) };

        // populate the latest valid hash field
        Ok(Some(self.prepare_invalid_response(header.parent_hash)?))
    }

    /// Record latency metrics for one call to make a block canonical
    /// Takes start time of the call and result of the make canonical call
    ///
    /// Handles cases for error, already canonical and committed blocks
    fn record_make_canonical_latency(
        &self,
        start: Instant,
        outcome: &Result<CanonicalOutcome, CanonicalError>,
    ) -> Duration {
        let elapsed = start.elapsed();
        self.metrics.make_canonical_latency.record(elapsed);
        match outcome {
            Ok(CanonicalOutcome::AlreadyCanonical { .. }) => {
                self.metrics.make_canonical_already_canonical_latency.record(elapsed)
            }
            Ok(CanonicalOutcome::Committed { .. }) => {
                self.metrics.make_canonical_committed_latency.record(elapsed)
            }
            Err(_) => self.metrics.make_canonical_error_latency.record(elapsed),
        }
        elapsed
    }

    /// Ensures that the given forkchoice state is consistent, assuming the head block has been
    /// made canonical.
    ///
    /// If the forkchoice state is consistent, this will return Ok(None). Otherwise, this will
    /// return an instance of [`OnForkChoiceUpdated`] that is INVALID.
    ///
    /// This also updates the safe and finalized blocks in the [`CanonChainTracker`], if they are
    /// consistent with the head block.
    fn ensure_consistent_forkchoice_state(
        &self,
        state: ForkchoiceState,
    ) -> ProviderResult<Option<OnForkChoiceUpdated>> {
        // Ensure that the finalized block, if not zero, is known and in the canonical chain
        // after the head block is canonicalized.
        //
        // This ensures that the finalized block is consistent with the head block, i.e. the
        // finalized block is an ancestor of the head block.
        if !state.finalized_block_hash.is_zero() &&
            !self.blockchain.is_canonical(state.finalized_block_hash)?
        {
            return Ok(Some(OnForkChoiceUpdated::invalid_state()))
        }

        // Finalized block is consistent, so update it in the canon chain tracker.
        self.update_finalized_block(state.finalized_block_hash)?;

        // Also ensure that the safe block, if not zero, is known and in the canonical chain
        // after the head block is canonicalized.
        //
        // This ensures that the safe block is consistent with the head block, i.e. the safe
        // block is an ancestor of the head block.
        if !state.safe_block_hash.is_zero() &&
            !self.blockchain.is_canonical(state.safe_block_hash)?
        {
            return Ok(Some(OnForkChoiceUpdated::invalid_state()))
        }

        // Safe block is consistent, so update it in the canon chain tracker.
        self.update_safe_block(state.safe_block_hash)?;

        Ok(None)
    }

    /// Sets the state of the canon chain tracker based to the given head.
    ///
    /// This expects the given head to be the new canonical head.
    ///
    /// Additionally, updates the head used for p2p handshakes.
    ///
    /// This also updates the tracked safe and finalized blocks, and should be called before
    /// returning a VALID forkchoice update response
    fn update_canon_chain(&self, head: SealedHeader, update: &ForkchoiceState) -> RethResult<()> {
        self.update_head(head)?;
        self.update_finalized_block(update.finalized_block_hash)?;
        self.update_safe_block(update.safe_block_hash)?;
        Ok(())
    }

    /// Updates the state of the canon chain tracker based on the given head.
    ///
    /// This expects the given head to be the new canonical head.
    /// Additionally, updates the head used for p2p handshakes.
    ///
    /// This should be called before returning a VALID forkchoice update response
    #[inline]
    fn update_head(&self, head: SealedHeader) -> RethResult<()> {
        let mut head_block = Head {
            number: head.number,
            hash: head.hash(),
            difficulty: head.difficulty,
            timestamp: head.timestamp,
            // NOTE: this will be set later
            total_difficulty: Default::default(),
        };

        // we update the tracked header first
        self.blockchain.set_canonical_head(head);

        head_block.total_difficulty =
            self.blockchain.header_td_by_number(head_block.number)?.ok_or_else(|| {
                RethError::Provider(ProviderError::TotalDifficultyNotFound(head_block.number))
            })?;
        self.sync_state_updater.update_status(head_block);

        Ok(())
    }

    /// Updates the tracked safe block if we have it
    ///
    /// Returns an error if the block is not found.
    #[inline]
    fn update_safe_block(&self, safe_block_hash: B256) -> ProviderResult<()> {
        if !safe_block_hash.is_zero() {
            if self.blockchain.safe_block_hash()? == Some(safe_block_hash) {
                // nothing to update
                return Ok(())
            }

            let safe = self
                .blockchain
                .find_block_by_hash(safe_block_hash, BlockSource::Any)?
                .ok_or_else(|| ProviderError::UnknownBlockHash(safe_block_hash))?;
            self.blockchain.set_safe(safe.header.seal(safe_block_hash));
        }
        Ok(())
    }

    /// Updates the tracked finalized block if we have it
    ///
    /// Returns an error if the block is not found.
    #[inline]
    fn update_finalized_block(&self, finalized_block_hash: B256) -> ProviderResult<()> {
        if !finalized_block_hash.is_zero() {
            if self.blockchain.finalized_block_hash()? == Some(finalized_block_hash) {
                // nothing to update
                return Ok(())
            }

            let finalized = self
                .blockchain
                .find_block_by_hash(finalized_block_hash, BlockSource::Any)?
                .ok_or_else(|| ProviderError::UnknownBlockHash(finalized_block_hash))?;
            self.blockchain.finalize_block(finalized.number)?;
            self.blockchain.set_finalized(finalized.header.seal(finalized_block_hash));
        }
        Ok(())
    }

    /// Handler for a failed a forkchoice update due to a canonicalization error.
    ///
    /// This will determine if the state's head is invalid, and if so, return immediately.
    ///
    /// If the newest head is not invalid, then this will trigger a new pipeline run to sync the gap
    ///
    /// See [`Self::on_forkchoice_updated`] and [`BlockchainTreeEngine::make_canonical`].
    fn on_failed_canonical_forkchoice_update(
        &mut self,
        state: &ForkchoiceState,
        error: CanonicalError,
    ) -> ProviderResult<PayloadStatus> {
        debug_assert!(self.sync.is_pipeline_idle(), "pipeline must be idle");

        // check if the new head was previously invalidated, if so then we deem this FCU
        // as invalid
        if let Some(invalid_ancestor) = self.check_invalid_ancestor(state.head_block_hash)? {
            warn!(target: "consensus::engine", %error, ?state, ?invalid_ancestor, head=?state.head_block_hash, "Failed to canonicalize the head hash, head is also considered invalid");
            debug!(target: "consensus::engine", head=?state.head_block_hash, current_error=%error, "Head was previously marked as invalid");
            return Ok(invalid_ancestor)
        }

        match &error {
            CanonicalError::Validation(BlockValidationError::BlockPreMerge { .. }) => {
                warn!(target: "consensus::engine", %error, ?state, "Failed to canonicalize the head hash");
                return Ok(PayloadStatus::from_status(PayloadStatusEnum::Invalid {
                    validation_error: error.to_string(),
                })
                .with_latest_valid_hash(B256::ZERO))
            }
            CanonicalError::BlockchainTree(BlockchainTreeError::BlockHashNotFoundInChain {
                ..
            }) => {
                // This just means we couldn't find the block when attempting to make it canonical,
                // so we should not warn the user, since this will result in us attempting to sync
                // to a new target and is considered normal operation during sync
            }
            CanonicalError::OptimisticTargetRevert(block_number) => {
                self.sync.set_pipeline_sync_target(PipelineTarget::Unwind(*block_number));
                return Ok(PayloadStatus::from_status(PayloadStatusEnum::Syncing))
            }
            _ => {
                warn!(target: "consensus::engine", %error, ?state, "Failed to canonicalize the head hash");
                // TODO(mattsse) better error handling before attempting to sync (FCU could be
                // invalid): only trigger sync if we can't determine whether the FCU is invalid
            }
        }

        // we assume the FCU is valid and at least the head is missing,
        // so we need to start syncing to it
        //
        // find the appropriate target to sync to, if we don't have the safe block hash then we
        // start syncing to the safe block via pipeline first
        let target = if self.forkchoice_state_tracker.is_empty() &&
            // check that safe block is valid and missing
            !state.safe_block_hash.is_zero() &&
            self.blockchain.block_number(state.safe_block_hash).ok().flatten().is_none()
        {
            state.safe_block_hash
        } else {
            state.head_block_hash
        };

        // we need to first check the buffer for the target and its ancestors
        let target = self.lowest_buffered_ancestor_or(target);

        // if the threshold is zero, we should not download the block first, and just use the
        // pipeline. Otherwise we use the tree to insert the block first
        if self.pipeline_run_threshold == 0 {
            // use the pipeline to sync to the target
            trace!(target: "consensus::engine", %target, "Triggering pipeline run to sync missing ancestors of the new head");
            self.sync.set_pipeline_sync_target(target.into());
        } else {
            // trigger a full block download for missing hash, or the parent of its lowest buffered
            // ancestor
            trace!(target: "consensus::engine", request=%target, "Triggering full block download for missing ancestors of the new head");
            self.sync.download_full_block(target);
        }

        debug!(target: "consensus::engine", %target, "Syncing to new target");
        Ok(PayloadStatus::from_status(PayloadStatusEnum::Syncing))
    }

    /// Return the parent hash of the lowest buffered ancestor for the requested block, if there
    /// are any buffered ancestors. If there are no buffered ancestors, and the block itself does
    /// not exist in the buffer, this returns the hash that is passed in.
    ///
    /// Returns the parent hash of the block itself if the block is buffered and has no other
    /// buffered ancestors.
    fn lowest_buffered_ancestor_or(&self, hash: B256) -> B256 {
        self.blockchain
            .lowest_buffered_ancestor(hash)
            .map(|block| block.parent_hash)
            .unwrap_or_else(|| hash)
    }

    /// When the Consensus layer receives a new block via the consensus gossip protocol,
    /// the transactions in the block are sent to the execution layer in the form of a
    /// [`ExecutionPayload`]. The Execution layer executes the transactions and validates the
    /// state in the block header, then passes validation data back to Consensus layer, that
    /// adds the block to the head of its own blockchain and attests to it. The block is then
    /// broadcast over the consensus p2p network in the form of a "Beacon block".
    ///
    /// These responses should adhere to the [Engine API Spec for
    /// `engine_newPayload`](https://github.com/ethereum/execution-apis/blob/main/src/engine/paris.md#specification).
    ///
    /// This returns a [`PayloadStatus`] that represents the outcome of a processed new payload and
    /// returns an error if an internal error occurred.
    #[instrument(level = "trace", skip(self, payload, cancun_fields), fields(block_hash = ?payload.block_hash(), block_number = %payload.block_number(), is_pipeline_idle = %self.sync.is_pipeline_idle()), target = "consensus::engine")]
    fn on_new_payload(
        &mut self,
        payload: ExecutionPayload,
        cancun_fields: Option<CancunPayloadFields>,
    ) -> Result<Either<PayloadStatus, SealedBlock>, BeaconOnNewPayloadError> {
        self.metrics.new_payload_messages.increment(1);

        // Ensures that the given payload does not violate any consensus rules that concern the
        // block's layout, like:
        //    - missing or invalid base fee
        //    - invalid extra data
        //    - invalid transactions
        //    - incorrect hash
        //    - the versioned hashes passed with the payload do not exactly match transaction
        //      versioned hashes
        //    - the block does not contain blob transactions if it is pre-cancun
        //
        // This validates the following engine API rule:
        //
        // 3. Given the expected array of blob versioned hashes client software **MUST** run its
        //    validation by taking the following steps:
        //
        //   1. Obtain the actual array by concatenating blob versioned hashes lists
        //      (`tx.blob_versioned_hashes`) of each [blob
        //      transaction](https://eips.ethereum.org/EIPS/eip-4844#new-transaction-type) included
        //      in the payload, respecting the order of inclusion. If the payload has no blob
        //      transactions the expected array **MUST** be `[]`.
        //
        //   2. Return `{status: INVALID, latestValidHash: null, validationError: errorMessage |
        //      null}` if the expected and the actual arrays don't match.
        //
        // This validation **MUST** be instantly run in all cases even during active sync process.
        let parent_hash = payload.parent_hash();
        let block = match self
            .payload_validator
            .ensure_well_formed_payload(payload, cancun_fields.into())
        {
            Ok(block) => block,
            Err(error) => {
                error!(target: "consensus::engine", %error, "Invalid payload");
                // we need to convert the error to a payload status (response to the CL)

                let latest_valid_hash =
                    if error.is_block_hash_mismatch() || error.is_invalid_versioned_hashes() {
                        // Engine-API rules:
                        // > `latestValidHash: null` if the blockHash validation has failed (<https://github.com/ethereum/execution-apis/blob/fe8e13c288c592ec154ce25c534e26cb7ce0530d/src/engine/shanghai.md?plain=1#L113>)
                        // > `latestValidHash: null` if the expected and the actual arrays don't match (<https://github.com/ethereum/execution-apis/blob/fe8e13c288c592ec154ce25c534e26cb7ce0530d/src/engine/cancun.md?plain=1#L103>)
                        None
                    } else {
                        self.latest_valid_hash_for_invalid_payload(parent_hash)
                            .map_err(BeaconOnNewPayloadError::internal)?
                    };

                let status = PayloadStatusEnum::from(error);
                return Ok(Either::Left(PayloadStatus::new(status, latest_valid_hash)))
            }
        };

        let mut lowest_buffered_ancestor = self.lowest_buffered_ancestor_or(block.hash());
        if lowest_buffered_ancestor == block.hash() {
            lowest_buffered_ancestor = block.parent_hash;
        }

        // now check the block itself
        if let Some(status) = self
            .check_invalid_ancestor_with_head(lowest_buffered_ancestor, block.hash())
            .map_err(BeaconOnNewPayloadError::internal)?
        {
            Ok(Either::Left(status))
        } else {
            Ok(Either::Right(block))
        }
    }

    /// Validates the payload attributes with respect to the header and fork choice state.
    ///
    /// Note: At this point, the fork choice update is considered to be VALID, however, we can still
    /// return an error if the payload attributes are invalid.
    fn process_payload_attributes(
        &self,
        attrs: EngineT::PayloadAttributes,
        head: Header,
        state: ForkchoiceState,
    ) -> OnForkChoiceUpdated {
        // 7. Client software MUST ensure that payloadAttributes.timestamp is greater than timestamp
        //    of a block referenced by forkchoiceState.headBlockHash. If this condition isn't held
        //    client software MUST respond with -38003: `Invalid payload attributes` and MUST NOT
        //    begin a payload build process. In such an event, the forkchoiceState update MUST NOT
        //    be rolled back.
        if attrs.timestamp() <= head.timestamp {
            return OnForkChoiceUpdated::invalid_payload_attributes()
        }

        // 8. Client software MUST begin a payload build process building on top of
        //    forkchoiceState.headBlockHash and identified via buildProcessId value if
        //    payloadAttributes is not null and the forkchoice state has been updated successfully.
        //    The build process is specified in the Payload building section.
        match <EngineT::PayloadBuilderAttributes as PayloadBuilderAttributes>::try_new(
            state.head_block_hash,
            attrs,
        ) {
            Ok(attributes) => {
                // send the payload to the builder and return the receiver for the pending payload
                // id, initiating payload job is handled asynchronously
                let pending_payload_id = self.payload_builder.send_new_payload(attributes);

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
            Err(_) => OnForkChoiceUpdated::invalid_payload_attributes(),
        }
    }

    /// When the pipeline is active, the tree is unable to commit any additional blocks since the
    /// pipeline holds exclusive access to the database.
    ///
    /// In this scenario we buffer the payload in the tree if the payload is valid, once the
    /// pipeline is finished, the tree is then able to also use the buffered payloads to commit to a
    /// (newer) canonical chain.
    ///
    /// This will return `SYNCING` if the block was buffered successfully, and an error if an error
    /// occurred while buffering the block.
    #[instrument(level = "trace", skip_all, target = "consensus::engine", ret)]
    fn try_buffer_payload(
        &mut self,
        block: SealedBlock,
    ) -> Result<PayloadStatus, InsertBlockError> {
        self.blockchain.buffer_block_without_senders(block)?;
        Ok(PayloadStatus::from_status(PayloadStatusEnum::Syncing))
    }

    /// Attempts to insert a new payload into the tree.
    ///
    /// Caution: This expects that the pipeline is idle.
    #[instrument(level = "trace", skip_all, target = "consensus::engine", ret)]
    fn try_insert_new_payload(
        &mut self,
        block: SealedBlock,
    ) -> Result<PayloadStatus, InsertBlockError> {
        debug_assert!(self.sync.is_pipeline_idle(), "pipeline must be idle");

        let block_hash = block.hash();
        let start = Instant::now();
        let status = self
            .blockchain
            .insert_block_without_senders(block.clone(), BlockValidationKind::Exhaustive)?;

        let elapsed = start.elapsed();
        let mut latest_valid_hash = None;
        let status = match status {
            InsertPayloadOk::Inserted(BlockStatus::Valid(attachment)) => {
                latest_valid_hash = Some(block_hash);
                let block = Arc::new(block);
                let event = if attachment.is_canonical() {
                    BeaconConsensusEngineEvent::CanonicalBlockAdded(block, elapsed)
                } else {
                    BeaconConsensusEngineEvent::ForkBlockAdded(block)
                };
                self.event_sender.notify(event);
                PayloadStatusEnum::Valid
            }
            InsertPayloadOk::AlreadySeen(BlockStatus::Valid(_)) => {
                latest_valid_hash = Some(block_hash);
                PayloadStatusEnum::Valid
            }
            InsertPayloadOk::Inserted(BlockStatus::Disconnected { .. }) |
            InsertPayloadOk::AlreadySeen(BlockStatus::Disconnected { .. }) => {
                // check if the block's parent is already marked as invalid
                if let Some(status) =
                    self.check_invalid_ancestor_with_head(block.parent_hash, block.hash()).map_err(
                        |error| InsertBlockError::new(block, InsertBlockErrorKind::Provider(error)),
                    )?
                {
                    return Ok(status)
                }

                // not known to be invalid, but we don't know anything else
                PayloadStatusEnum::Syncing
            }
        };
        Ok(PayloadStatus::new(status, latest_valid_hash))
    }

    /// This handles downloaded blocks that are shown to be disconnected from the canonical chain.
    ///
    /// This mainly compares the missing parent of the downloaded block with the current canonical
    /// tip, and decides whether or not the pipeline should be run.
    ///
    /// The canonical tip is compared to the missing parent using `exceeds_pipeline_run_threshold`,
    /// which returns true if the missing parent is sufficiently ahead of the canonical tip. If so,
    /// the pipeline is run. Otherwise, we need to insert blocks using the blockchain tree, and
    /// must download blocks outside of the pipeline. In this case, the distance is used to
    /// determine how many blocks we should download at once.
    fn on_disconnected_block(
        &mut self,
        downloaded_block: BlockNumHash,
        missing_parent: BlockNumHash,
        head: BlockNumHash,
    ) {
        // compare the missing parent with the canonical tip
        if let Some(target) = self.can_pipeline_sync_to_finalized(
            head.number,
            missing_parent.number,
            Some(downloaded_block),
        ) {
            // we don't have the block yet and the distance exceeds the allowed
            // threshold
            self.sync.set_pipeline_sync_target(target.into());
            // we can exit early here because the pipeline will take care of syncing
            return
        }

        // continue downloading the missing parent
        //
        // this happens if either:
        //  * the missing parent block num < canonical tip num
        //    * this case represents a missing block on a fork that is shorter than the canonical
        //      chain
        //  * the missing parent block num >= canonical tip num, but the number of missing blocks is
        //    less than the pipeline threshold
        //    * this case represents a potentially long range of blocks to download and execute
        if let Some(distance) = self.distance_from_local_tip(head.number, missing_parent.number) {
            self.sync.download_block_range(missing_parent.hash, distance)
        } else {
            // This happens when the missing parent is on an outdated
            // sidechain
            self.sync.download_full_block(missing_parent.hash);
        }
    }

    /// Attempt to form a new canonical chain based on the current sync target.
    ///
    /// This is invoked when we successfully __downloaded__ a new block from the network which
    /// resulted in [`BlockStatus::Valid`].
    ///
    /// Note: This will not succeed if the sync target has changed since the block download request
    /// was issued and the new target is still disconnected and additional missing blocks are
    /// downloaded
    fn try_make_sync_target_canonical(
        &mut self,
        inserted: BlockNumHash,
    ) -> Result<(), (B256, CanonicalError)> {
        let Some(target) = self.forkchoice_state_tracker.sync_target_state() else { return Ok(()) };

        // optimistically try to make the head of the current FCU target canonical, the sync
        // target might have changed since the block download request was issued
        // (new FCU received)
        let start = Instant::now();
        let make_canonical_result = self.blockchain.make_canonical(target.head_block_hash);
        let elapsed = self.record_make_canonical_latency(start, &make_canonical_result);
        match make_canonical_result {
            Ok(outcome) => {
                if let CanonicalOutcome::Committed { head } = &outcome {
                    self.event_sender.notify(BeaconConsensusEngineEvent::CanonicalChainCommitted(
                        Box::new(head.clone()),
                        elapsed,
                    ));
                }

                let new_head = outcome.into_header();
                debug!(target: "consensus::engine", hash=?new_head.hash(), number=new_head.number, "Canonicalized new head");

                // we can update the FCU blocks
                if let Err(err) = self.update_canon_chain(new_head, &target) {
                    debug!(target: "consensus::engine", ?err, ?target, "Failed to update the canonical chain tracker");
                }

                // we're no longer syncing
                self.sync_state_updater.update_sync_state(SyncState::Idle);

                // clear any active block requests
                self.sync.clear_block_download_requests();
                Ok(())
            }
            Err(err) => {
                // if we failed to make the FCU's head canonical, because we don't have that
                // block yet, then we can try to make the inserted block canonical if we know
                // it's part of the canonical chain: if it's the safe or the finalized block
                if err.is_block_hash_not_found() {
                    // if the inserted block is the currently targeted `finalized` or `safe`
                    // block, we will attempt to make them canonical,
                    // because they are also part of the canonical chain and
                    // their missing block range might already be downloaded (buffered).
                    if let Some(target_hash) =
                        ForkchoiceStateHash::find(&target, inserted.hash).filter(|h| !h.is_head())
                    {
                        // TODO: do not ignore this
                        let _ = self.blockchain.make_canonical(*target_hash.as_ref());
                    }
                } else if let Some(block_number) = err.optimistic_revert_block_number() {
                    self.sync.set_pipeline_sync_target(PipelineTarget::Unwind(block_number));
                }

                Err((target.head_block_hash, err))
            }
        }
    }

    /// Event handler for events emitted by the [`EngineSyncController`].
    ///
    /// This returns a result to indicate whether the engine future should resolve (fatal error).
    fn on_sync_event(
        &mut self,
        event: EngineSyncEvent,
    ) -> Result<EngineEventOutcome, BeaconConsensusEngineError> {
        let outcome = match event {
            EngineSyncEvent::FetchedFullBlock(block) => {
                trace!(target: "consensus::engine", hash=?block.hash(), number=%block.number, "Downloaded full block");
                // Insert block only if the block's parent is not marked as invalid
                if self
                    .check_invalid_ancestor_with_head(block.parent_hash, block.hash())
                    .map_err(|error| BeaconConsensusEngineError::Common(error.into()))?
                    .is_none()
                {
                    self.set_blockchain_tree_action(
                        BlockchainTreeAction::InsertDownloadedPayload { block },
                    );
                }
                EngineEventOutcome::Processed
            }
            EngineSyncEvent::PipelineStarted(target) => {
                trace!(target: "consensus::engine", ?target, continuous = target.is_none(), "Started the pipeline");
                self.metrics.pipeline_runs.increment(1);
                self.sync_state_updater.update_sync_state(SyncState::Syncing);
                EngineEventOutcome::Processed
            }
            EngineSyncEvent::PipelineFinished { result, reached_max_block } => {
                trace!(target: "consensus::engine", ?result, ?reached_max_block, "Pipeline finished");
                // Any pipeline error at this point is fatal.
                let ctrl = result?;
                if reached_max_block {
                    // Terminate the sync early if it's reached the maximum user-configured block.
                    EngineEventOutcome::ReachedMaxBlock
                } else {
                    self.on_pipeline_outcome(ctrl)?;
                    EngineEventOutcome::Processed
                }
            }
            EngineSyncEvent::PipelineTaskDropped => {
                error!(target: "consensus::engine", "Failed to receive spawned pipeline");
                return Err(BeaconConsensusEngineError::PipelineChannelClosed)
            }
        };

        Ok(outcome)
    }

    /// Invoked when the pipeline has successfully finished.
    ///
    /// Updates the internal sync state depending on the pipeline configuration,
    /// the outcome of the pipeline run and the last observed forkchoice state.
    fn on_pipeline_outcome(&mut self, ctrl: ControlFlow) -> RethResult<()> {
        // Pipeline unwound, memorize the invalid block and wait for CL for next sync target.
        if let ControlFlow::Unwind { bad_block, .. } = ctrl {
            warn!(target: "consensus::engine", invalid_hash=?bad_block.hash(), invalid_number=?bad_block.number, "Bad block detected in unwind");
            // update the `invalid_headers` cache with the new invalid header
            self.invalid_headers.insert(*bad_block);
            return Ok(())
        }

        let sync_target_state = match self.forkchoice_state_tracker.sync_target_state() {
            Some(current_state) => current_state,
            None => {
                // This is only possible if the node was run with `debug.tip`
                // argument and without CL.
                warn!(target: "consensus::engine", "No fork choice state available");
                return Ok(())
            }
        };

        if sync_target_state.finalized_block_hash.is_zero() {
            self.set_canonical_head(ctrl.block_number().unwrap_or_default())?;
            self.blockchain.update_block_hashes_and_clear_buffered()?;
            self.blockchain.connect_buffered_blocks_to_canonical_hashes()?;
            // We are on an optimistic syncing process, better to wait for the next FCU to handle
            return Ok(())
        }

        // Next, we check if we need to schedule another pipeline run or transition
        // to live sync via tree.
        // This can arise if we buffer the forkchoice head, and if the head is an
        // ancestor of an invalid block.
        //
        //  * The forkchoice head could be buffered if it were first sent as a `newPayload` request.
        //
        // In this case, we won't have the head hash in the database, so we would
        // set the pipeline sync target to a known-invalid head.
        //
        // This is why we check the invalid header cache here.
        let lowest_buffered_ancestor =
            self.lowest_buffered_ancestor_or(sync_target_state.head_block_hash);

        // this inserts the head into invalid headers cache
        // if the lowest buffered ancestor is invalid
        if self
            .check_invalid_ancestor_with_head(
                lowest_buffered_ancestor,
                sync_target_state.head_block_hash,
            )?
            .is_some()
        {
            warn!(
                target: "consensus::engine",
                invalid_ancestor = %lowest_buffered_ancestor,
                head = %sync_target_state.head_block_hash,
                "Current head has an invalid ancestor"
            );
            return Ok(())
        }

        // get the block number of the finalized block, if we have it
        let newest_finalized = self
            .blockchain
            .buffered_header_by_hash(sync_target_state.finalized_block_hash)
            .map(|header| header.number);

        // The block number that the pipeline finished at - if the progress or newest
        // finalized is None then we can't check the distance anyways.
        //
        // If both are Some, we perform another distance check and return the desired
        // pipeline target
        let pipeline_target =
            ctrl.block_number().zip(newest_finalized).and_then(|(progress, finalized_number)| {
                // Determines whether or not we should run the pipeline again, in case
                // the new gap is large enough to warrant
                // running the pipeline.
                self.can_pipeline_sync_to_finalized(progress, finalized_number, None)
            });

        // If the distance is large enough, we should run the pipeline again to prevent
        // the tree update from executing too many blocks and blocking.
        if let Some(target) = pipeline_target {
            // run the pipeline to the target since the distance is sufficient
            self.sync.set_pipeline_sync_target(target.into());
        } else if let Some(number) =
            self.blockchain.block_number(sync_target_state.finalized_block_hash)?
        {
            // Finalized block is in the database, attempt to restore the tree with
            // the most recent canonical hashes.
            self.blockchain.connect_buffered_blocks_to_canonical_hashes_and_finalize(number).inspect_err(|error| {
                error!(target: "consensus::engine", %error, "Error restoring blockchain tree state");
            })?;
        } else {
            // We don't have the finalized block in the database, so we need to
            // trigger another pipeline run.
            self.sync.set_pipeline_sync_target(sync_target_state.finalized_block_hash.into());
        }

        Ok(())
    }

    fn set_canonical_head(&self, max_block: BlockNumber) -> RethResult<()> {
        let max_header = self.blockchain.sealed_header(max_block)
        .inspect_err(|error| {
            error!(target: "consensus::engine", %error, "Error getting canonical header for continuous sync");
        })?
        .ok_or_else(|| ProviderError::HeaderNotFound(max_block.into()))?;
        self.blockchain.set_canonical_head(max_header);

        Ok(())
    }

    fn on_hook_result(&self, polled_hook: PolledHook) -> Result<(), BeaconConsensusEngineError> {
        if let EngineHookEvent::Finished(Err(error)) = &polled_hook.event {
            error!(
                target: "consensus::engine",
                name = %polled_hook.name,
                ?error,
                "Hook finished with error"
            )
        }

        if polled_hook.db_access_level.is_read_write() {
            match polled_hook.event {
                EngineHookEvent::NotReady => {}
                EngineHookEvent::Started => {
                    // If the hook has read-write access to the database, it means that the engine
                    // can't process any FCU messages from CL. To prevent CL from sending us
                    // unneeded updates, we need to respond `true` on `eth_syncing` request.
                    self.sync_state_updater.update_sync_state(SyncState::Syncing)
                }
                EngineHookEvent::Finished(_) => {
                    // Hook with read-write access to the database has finished running, so engine
                    // can process new FCU messages from CL again. It's safe to
                    // return `false` on `eth_syncing` request.
                    self.sync_state_updater.update_sync_state(SyncState::Idle);
                    // If the hook had read-write access to the database, it means that the engine
                    // may have accumulated some buffered blocks.
                    if let Err(error) =
                        self.blockchain.connect_buffered_blocks_to_canonical_hashes()
                    {
                        error!(target: "consensus::engine", %error, "Error connecting buffered blocks to canonical hashes on hook result");
                        return Err(RethError::Canonical(error).into())
                    }
                }
            }
        }

        Ok(())
    }

    /// Process the next set blockchain tree action.
    /// The handler might set next blockchain tree action to perform,
    /// so the state change should be handled accordingly.
    fn on_blockchain_tree_action(
        &mut self,
        action: BlockchainTreeAction<EngineT>,
    ) -> RethResult<EngineEventOutcome> {
        match action {
            BlockchainTreeAction::MakeForkchoiceHeadCanonical { state, attrs, tx } => {
                let start = Instant::now();
                let result = self.blockchain.make_canonical(state.head_block_hash);
                let elapsed = self.record_make_canonical_latency(start, &result);
                match self
                    .on_forkchoice_updated_make_canonical_result(state, attrs, result, elapsed)
                {
                    Ok(on_updated) => {
                        trace!(target: "consensus::engine", status = ?on_updated, ?state, "Returning forkchoice status");
                        let fcu_status = on_updated.forkchoice_status();
                        self.on_forkchoice_updated_status(state, on_updated, tx);

                        if fcu_status.is_valid() {
                            let tip_number = self.blockchain.canonical_tip().number;
                            if self.sync.has_reached_max_block(tip_number) {
                                // Terminate the sync early if it's reached
                                // the maximum user configured block.
                                return Ok(EngineEventOutcome::ReachedMaxBlock)
                            }
                        }
                    }
                    Err(error) => {
                        let _ = tx.send(Err(RethError::Canonical(error.clone())));
                        if error.is_fatal() {
                            return Err(RethError::Canonical(error))
                        }
                    }
                };
            }
            BlockchainTreeAction::InsertNewPayload { block, tx } => {
                let block_hash = block.hash();
                let block_num_hash = block.num_hash();
                let result = if self.sync.is_pipeline_idle() {
                    // we can only insert new payloads if the pipeline is _not_ running, because it
                    // holds exclusive access to the database
                    self.try_insert_new_payload(block)
                } else {
                    self.try_buffer_payload(block)
                };

                let status = match result {
                    Ok(status) => status,
                    Err(error) => {
                        warn!(target: "consensus::engine", %error, "Error while processing payload");

                        let (block, error) = error.split();
                        if !error.is_invalid_block() {
                            // TODO: revise if any error should be considered fatal at this point.
                            let _ =
                                tx.send(Err(BeaconOnNewPayloadError::Internal(Box::new(error))));
                            return Ok(EngineEventOutcome::Processed)
                        }

                        // If the error was due to an invalid payload, the payload is added to the
                        // invalid headers cache and `Ok` with [PayloadStatusEnum::Invalid] is
                        // returned.
                        warn!(target: "consensus::engine", invalid_hash=?block.hash(), invalid_number=?block.number, %error, "Invalid block error on new payload");
                        let latest_valid_hash = if error.is_block_pre_merge() {
                            // zero hash must be returned if block is pre-merge
                            Some(B256::ZERO)
                        } else {
                            self.latest_valid_hash_for_invalid_payload(block.parent_hash)?
                        };
                        // keep track of the invalid header
                        self.invalid_headers.insert(block.header);
                        PayloadStatus::new(
                            PayloadStatusEnum::Invalid { validation_error: error.to_string() },
                            latest_valid_hash,
                        )
                    }
                };

                if status.is_valid() {
                    if let Some(target) = self.forkchoice_state_tracker.sync_target_state() {
                        // if we're currently syncing and the inserted block is the targeted
                        // FCU head block, we can try to make it canonical.
                        if block_hash == target.head_block_hash {
                            self.set_blockchain_tree_action(
                                BlockchainTreeAction::MakeNewPayloadCanonical {
                                    payload_num_hash: block_num_hash,
                                    status,
                                    tx,
                                },
                            );
                            return Ok(EngineEventOutcome::Processed)
                        }
                    }
                    // block was successfully inserted, so we can cancel the full block
                    // request, if any exists
                    self.sync.cancel_full_block_request(block_hash);
                }

                trace!(target: "consensus::engine", ?status, "Returning payload status");
                let _ = tx.send(Ok(status));
            }
            BlockchainTreeAction::MakeNewPayloadCanonical { payload_num_hash, status, tx } => {
                let status = match self.try_make_sync_target_canonical(payload_num_hash) {
                    Ok(()) => status,
                    Err((_hash, error)) => {
                        if error.is_fatal() {
                            let response =
                                Err(BeaconOnNewPayloadError::Internal(Box::new(error.clone())));
                            let _ = tx.send(response);
                            return Err(RethError::Canonical(error))
                        } else if error.optimistic_revert_block_number().is_some() {
                            // engine already set the pipeline unwind target on
                            // `try_make_sync_target_canonical`
                            PayloadStatus::from_status(PayloadStatusEnum::Syncing)
                        } else {
                            // If we could not make the sync target block canonical,
                            // we should return the error as an invalid payload status.
                            PayloadStatus::new(
                                PayloadStatusEnum::Invalid { validation_error: error.to_string() },
                                // TODO: return a proper latest valid hash
                                // See: <https://github.com/paradigmxyz/reth/issues/7146>
                                self.forkchoice_state_tracker.last_valid_head(),
                            )
                        }
                    }
                };

                trace!(target: "consensus::engine", ?status, "Returning payload status");
                let _ = tx.send(Ok(status));
            }

            BlockchainTreeAction::InsertDownloadedPayload { block } => {
                let downloaded_num_hash = block.num_hash();
                match self.blockchain.insert_block_without_senders(
                    block,
                    BlockValidationKind::SkipStateRootValidation,
                ) {
                    Ok(status) => {
                        match status {
                            InsertPayloadOk::Inserted(BlockStatus::Valid(_)) => {
                                // block is connected to the canonical chain and is valid.
                                // if it's not connected to current canonical head, the state root
                                // has not been validated.
                                if let Err((hash, error)) =
                                    self.try_make_sync_target_canonical(downloaded_num_hash)
                                {
                                    if error.is_fatal() {
                                        error!(target: "consensus::engine", %error, "Encountered fatal error while making sync target canonical: {:?}, {:?}", error, hash);
                                    } else if !error.is_block_hash_not_found() {
                                        debug!(
                                            target: "consensus::engine",
                                            "Unexpected error while making sync target canonical: {:?}, {:?}",
                                            error,
                                            hash
                                        )
                                    }
                                }
                            }
                            InsertPayloadOk::Inserted(BlockStatus::Disconnected {
                                head,
                                missing_ancestor: missing_parent,
                            }) => {
                                // block is not connected to the canonical head, we need to download
                                // its missing branch first
                                self.on_disconnected_block(
                                    downloaded_num_hash,
                                    missing_parent,
                                    head,
                                );
                            }
                            _ => (),
                        }
                    }
                    Err(err) => {
                        warn!(target: "consensus::engine", %err, "Failed to insert downloaded block");
                        if err.kind().is_invalid_block() {
                            let (block, err) = err.split();
                            warn!(target: "consensus::engine", invalid_number=?block.number, invalid_hash=?block.hash(), %err, "Marking block as invalid");

                            self.invalid_headers.insert(block.header);
                        }
                    }
                }
            }
        };
        Ok(EngineEventOutcome::Processed)
    }
}

/// On initialization, the consensus engine will poll the message receiver and return
/// [`Poll::Pending`] until the first forkchoice update message is received.
///
/// As soon as the consensus engine receives the first forkchoice updated message and updates the
/// local forkchoice state, it will launch the pipeline to sync to the head hash.
/// While the pipeline is syncing, the consensus engine will keep processing messages from the
/// receiver and forwarding them to the blockchain tree.
impl<DB, BT, Client, EngineT> Future for BeaconConsensusEngine<DB, BT, Client, EngineT>
where
    DB: Database + Unpin + 'static,
    Client: BlockClient + 'static,
    BT: BlockchainTreeEngine
        + BlockReader
        + BlockIdReader
        + CanonChainTracker
        + StageCheckpointReader
        + ChainSpecProvider
        + Unpin
        + 'static,
    EngineT: EngineTypes + Unpin,
{
    type Output = Result<(), BeaconConsensusEngineError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        // Control loop that advances the state
        'main: loop {
            // Poll a running hook with db write access (if any) and CL messages first, draining
            // both and then proceeding to polling other parts such as SyncController and hooks.
            loop {
                // Poll a running hook with db write access first, as we will not be able to process
                // any engine messages until it's finished.
                if let Poll::Ready(result) =
                    this.hooks.poll_active_db_write_hook(cx, this.current_engine_hook_context()?)?
                {
                    this.on_hook_result(result)?;
                    continue
                }

                // Process any blockchain tree action result as set forth during engine message
                // processing.
                if let Some(action) = this.blockchain_tree_action.take() {
                    match this.on_blockchain_tree_action(action) {
                        Ok(EngineEventOutcome::Processed) => {}
                        Ok(EngineEventOutcome::ReachedMaxBlock) => return Poll::Ready(Ok(())),
                        Err(error) => {
                            error!(target: "consensus::engine", %error, "Encountered fatal error");
                            return Poll::Ready(Err(error.into()))
                        }
                    };

                    // Blockchain tree action handler might set next action to take.
                    continue
                }

                // If the db write hook is no longer active and we have a pending forkchoice update,
                // process it first.
                if this.hooks.active_db_write_hook().is_none() {
                    if let Some((state, attrs, tx)) = this.pending_forkchoice_update.take() {
                        this.set_blockchain_tree_action(
                            BlockchainTreeAction::MakeForkchoiceHeadCanonical { state, attrs, tx },
                        );
                        continue
                    }
                }

                // Process one incoming message from the CL. We don't drain the messages right away,
                // because we want to sneak a polling of running hook in between them.
                //
                // These messages can affect the state of the SyncController and they're also time
                // sensitive, hence they are polled first.
                if let Poll::Ready(Some(msg)) = this.engine_message_stream.poll_next_unpin(cx) {
                    match msg {
                        BeaconEngineMessage::ForkchoiceUpdated { state, payload_attrs, tx } => {
                            this.on_forkchoice_updated(state, payload_attrs, tx);
                        }
                        BeaconEngineMessage::NewPayload { payload, cancun_fields, tx } => {
                            match this.on_new_payload(payload, cancun_fields) {
                                Ok(Either::Right(block)) => {
                                    this.set_blockchain_tree_action(
                                        BlockchainTreeAction::InsertNewPayload { block, tx },
                                    );
                                }
                                Ok(Either::Left(status)) => {
                                    let _ = tx.send(Ok(status));
                                }
                                Err(error) => {
                                    let _ = tx.send(Err(error));
                                }
                            }
                        }
                        BeaconEngineMessage::TransitionConfigurationExchanged => {
                            this.blockchain.on_transition_configuration_exchanged();
                        }
                    }
                    continue
                }

                // Both running hook with db write access and engine messages are pending,
                // proceed to other polls
                break
            }

            // process sync events if any
            if let Poll::Ready(sync_event) = this.sync.poll(cx) {
                match this.on_sync_event(sync_event)? {
                    // Sync event was successfully processed
                    EngineEventOutcome::Processed => (),
                    // Max block has been reached, exit the engine loop
                    EngineEventOutcome::ReachedMaxBlock => return Poll::Ready(Ok(())),
                }

                // this could have taken a while, so we start the next cycle to handle any new
                // engine messages
                continue 'main
            }

            // at this point, all engine messages and sync events are fully drained

            // Poll next hook if all conditions are met:
            // 1. Engine and sync messages are fully drained (both pending)
            // 2. Latest FCU status is not INVALID
            if !this.forkchoice_state_tracker.is_latest_invalid() {
                if let Poll::Ready(result) = this.hooks.poll_next_hook(
                    cx,
                    this.current_engine_hook_context()?,
                    this.sync.is_pipeline_active(),
                )? {
                    this.on_hook_result(result)?;

                    // ensure we're polling until pending while also checking for new engine
                    // messages before polling the next hook
                    continue 'main
                }
            }

            // incoming engine messages and sync events are drained, so we can yield back
            // control
            return Poll::Pending
        }
    }
}

enum BlockchainTreeAction<EngineT: EngineTypes> {
    MakeForkchoiceHeadCanonical {
        state: ForkchoiceState,
        attrs: Option<EngineT::PayloadAttributes>,
        tx: oneshot::Sender<RethResult<OnForkChoiceUpdated>>,
    },
    InsertNewPayload {
        block: SealedBlock,
        tx: oneshot::Sender<Result<PayloadStatus, BeaconOnNewPayloadError>>,
    },
    MakeNewPayloadCanonical {
        payload_num_hash: BlockNumHash,
        status: PayloadStatus,
        tx: oneshot::Sender<Result<PayloadStatus, BeaconOnNewPayloadError>>,
    },
    /// Action to insert a new block that we successfully downloaded from the network.
    /// There are several outcomes for inserting a downloaded block into the tree:
    ///
    /// ## [`BlockStatus::Valid`]
    ///
    /// The block is connected to the current canonical chain and is valid.
    /// If the block is an ancestor of the current forkchoice head, then we can try again to
    /// make the chain canonical.
    ///
    /// ## [`BlockStatus::Disconnected`]
    ///
    /// The block is not connected to the canonical chain, and we need to download the
    /// missing parent first.
    ///
    /// ## Insert Error
    ///
    /// If the insertion into the tree failed, then the block was well-formed (valid hash),
    /// but its chain is invalid, which means the FCU that triggered the
    /// download is invalid. Here we can stop because there's nothing to do here
    /// and the engine needs to wait for another FCU.
    InsertDownloadedPayload { block: SealedBlock },
}

/// Represents outcomes of processing an engine event
#[derive(Debug)]
enum EngineEventOutcome {
    /// Engine event was processed successfully, engine should continue.
    Processed,
    /// Engine event was processed successfully and reached max block.
    ReachedMaxBlock,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_utils::{spawn_consensus_engine, TestConsensusEngineBuilder},
        BeaconForkChoiceUpdateError,
    };
    use assert_matches::assert_matches;
    use reth_chainspec::{ChainSpecBuilder, MAINNET};
    use reth_provider::{BlockWriter, ProviderFactory};
    use reth_rpc_types::engine::{ForkchoiceState, ForkchoiceUpdated, PayloadStatus};
    use reth_rpc_types_compat::engine::payload::block_to_payload_v1;
    use reth_stages::{ExecOutput, PipelineError, StageError};
    use reth_stages_api::StageCheckpoint;
    use reth_testing_utils::generators::{self, Rng};
    use std::{collections::VecDeque, sync::Arc};
    use tokio::sync::oneshot::error::TryRecvError;

    // Pipeline error is propagated.
    #[tokio::test]
    async fn pipeline_error_is_propagated() {
        let mut rng = generators::rng();
        let chain_spec = Arc::new(
            ChainSpecBuilder::default()
                .chain(MAINNET.chain)
                .genesis(MAINNET.genesis.clone())
                .paris_activated()
                .build(),
        );

        let (consensus_engine, env) = TestConsensusEngineBuilder::new(chain_spec.clone())
            .with_pipeline_exec_outputs(VecDeque::from([Err(StageError::ChannelClosed)]))
            .disable_blockchain_tree_sync()
            .with_max_block(1)
            .build();

        let res = spawn_consensus_engine(consensus_engine);

        let _ = env
            .send_forkchoice_updated(ForkchoiceState {
                head_block_hash: rng.gen(),
                ..Default::default()
            })
            .await;
        assert_matches!(
            res.await,
            Ok(Err(BeaconConsensusEngineError::Pipeline(n))) if matches!(*n.as_ref(),PipelineError::Stage(StageError::ChannelClosed))
        );
    }

    // Test that the consensus engine is idle until first forkchoice updated is received.
    #[tokio::test]
    async fn is_idle_until_forkchoice_is_set() {
        let mut rng = generators::rng();
        let chain_spec = Arc::new(
            ChainSpecBuilder::default()
                .chain(MAINNET.chain)
                .genesis(MAINNET.genesis.clone())
                .paris_activated()
                .build(),
        );

        let (consensus_engine, env) = TestConsensusEngineBuilder::new(chain_spec.clone())
            .with_pipeline_exec_outputs(VecDeque::from([Err(StageError::ChannelClosed)]))
            .disable_blockchain_tree_sync()
            .with_max_block(1)
            .build();

        let mut rx = spawn_consensus_engine(consensus_engine);

        // consensus engine is idle
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_matches!(rx.try_recv(), Err(TryRecvError::Empty));

        // consensus engine is still idle because no FCUs were received
        let _ = env.send_new_payload(block_to_payload_v1(SealedBlock::default()), None).await;

        assert_matches!(rx.try_recv(), Err(TryRecvError::Empty));

        // consensus engine is still idle because pruning is running
        let _ = env
            .send_forkchoice_updated(ForkchoiceState {
                head_block_hash: rng.gen(),
                ..Default::default()
            })
            .await;
        assert_matches!(rx.try_recv(), Err(TryRecvError::Empty));

        // consensus engine receives a forkchoice state and triggers the pipeline when pruning is
        // finished
        loop {
            match rx.try_recv() {
                Ok(result) => {
                    assert_matches!(
                        result,
                        Err(BeaconConsensusEngineError::Pipeline(n)) if matches!(*n.as_ref(), PipelineError::Stage(StageError::ChannelClosed))
                    );
                    break
                }
                Err(TryRecvError::Empty) => {
                    let _ = env
                        .send_forkchoice_updated(ForkchoiceState {
                            head_block_hash: rng.gen(),
                            ..Default::default()
                        })
                        .await;
                }
                Err(err) => panic!("receive error: {err}"),
            }
        }
    }

    // Test that the consensus engine runs the pipeline again if the tree cannot be restored.
    // The consensus engine will propagate the second result (error) only if it runs the pipeline
    // for the second time.
    #[tokio::test]
    async fn runs_pipeline_again_if_tree_not_restored() {
        let mut rng = generators::rng();
        let chain_spec = Arc::new(
            ChainSpecBuilder::default()
                .chain(MAINNET.chain)
                .genesis(MAINNET.genesis.clone())
                .paris_activated()
                .build(),
        );

        let (consensus_engine, env) = TestConsensusEngineBuilder::new(chain_spec.clone())
            .with_pipeline_exec_outputs(VecDeque::from([
                Ok(ExecOutput { checkpoint: StageCheckpoint::new(1), done: true }),
                Err(StageError::ChannelClosed),
            ]))
            .disable_blockchain_tree_sync()
            .with_max_block(2)
            .build();

        let rx = spawn_consensus_engine(consensus_engine);

        let _ = env
            .send_forkchoice_updated(ForkchoiceState {
                head_block_hash: rng.gen(),
                finalized_block_hash: rng.gen(),
                ..Default::default()
            })
            .await;

        assert_matches!(
            rx.await,
            Ok(Err(BeaconConsensusEngineError::Pipeline(n)))  if matches!(*n.as_ref(),PipelineError::Stage(StageError::ChannelClosed))
        );
    }

    #[tokio::test]
    async fn terminates_upon_reaching_max_block() {
        let mut rng = generators::rng();
        let max_block = 1000;
        let chain_spec = Arc::new(
            ChainSpecBuilder::default()
                .chain(MAINNET.chain)
                .genesis(MAINNET.genesis.clone())
                .paris_activated()
                .build(),
        );

        let (consensus_engine, env) = TestConsensusEngineBuilder::new(chain_spec.clone())
            .with_pipeline_exec_outputs(VecDeque::from([Ok(ExecOutput {
                checkpoint: StageCheckpoint::new(max_block),
                done: true,
            })]))
            .with_max_block(max_block)
            .disable_blockchain_tree_sync()
            .build();

        let rx = spawn_consensus_engine(consensus_engine);

        let _ = env
            .send_forkchoice_updated(ForkchoiceState {
                head_block_hash: rng.gen(),
                ..Default::default()
            })
            .await;
        assert_matches!(rx.await, Ok(Ok(())));
    }

    fn insert_blocks<'a, DB: Database>(
        provider_factory: ProviderFactory<DB>,
        mut blocks: impl Iterator<Item = &'a SealedBlock>,
    ) {
        let provider = provider_factory.provider_rw().unwrap();
        blocks
            .try_for_each(|b| {
                provider
                    .insert_block(
                        b.clone().try_seal_with_senders().expect("invalid tx signature in block"),
                    )
                    .map(drop)
            })
            .expect("failed to insert");
        provider.commit().unwrap();
    }

    mod fork_choice_updated {
        use super::*;
        use reth_db::{tables, test_utils::create_test_static_files_dir};
        use reth_db_api::transaction::DbTxMut;
        use reth_primitives::U256;
        use reth_provider::providers::StaticFileProvider;
        use reth_rpc_types::engine::ForkchoiceUpdateError;
        use reth_testing_utils::generators::random_block;

        #[tokio::test]
        async fn empty_head() {
            let chain_spec = Arc::new(
                ChainSpecBuilder::default()
                    .chain(MAINNET.chain)
                    .genesis(MAINNET.genesis.clone())
                    .paris_activated()
                    .build(),
            );

            let (consensus_engine, env) = TestConsensusEngineBuilder::new(chain_spec.clone())
                .with_pipeline_exec_outputs(VecDeque::from([Ok(ExecOutput {
                    checkpoint: StageCheckpoint::new(0),
                    done: true,
                })]))
                .build();

            let mut engine_rx = spawn_consensus_engine(consensus_engine);

            let res = env.send_forkchoice_updated(ForkchoiceState::default()).await;
            assert_matches!(
                res,
                Err(BeaconForkChoiceUpdateError::ForkchoiceUpdateError(
                    ForkchoiceUpdateError::InvalidState
                ))
            );

            assert_matches!(engine_rx.try_recv(), Err(TryRecvError::Empty));
        }

        #[tokio::test]
        async fn valid_forkchoice() {
            let mut rng = generators::rng();
            let chain_spec = Arc::new(
                ChainSpecBuilder::default()
                    .chain(MAINNET.chain)
                    .genesis(MAINNET.genesis.clone())
                    .paris_activated()
                    .build(),
            );

            let (consensus_engine, env) = TestConsensusEngineBuilder::new(chain_spec.clone())
                .with_pipeline_exec_outputs(VecDeque::from([Ok(ExecOutput {
                    checkpoint: StageCheckpoint::new(0),
                    done: true,
                })]))
                .build();

            let genesis = random_block(&mut rng, 0, None, None, Some(0));
            let block1 = random_block(&mut rng, 1, Some(genesis.hash()), None, Some(0));
            let (_static_dir, static_dir_path) = create_test_static_files_dir();

            insert_blocks(
                ProviderFactory::new(
                    env.db.as_ref(),
                    chain_spec.clone(),
                    StaticFileProvider::read_write(static_dir_path).unwrap(),
                ),
                [&genesis, &block1].into_iter(),
            );
            env.db
                .update(|tx| {
                    tx.put::<tables::StageCheckpoints>(
                        StageId::Finish.to_string(),
                        StageCheckpoint::new(block1.number),
                    )
                })
                .unwrap()
                .unwrap();

            let mut engine_rx = spawn_consensus_engine(consensus_engine);

            let forkchoice = ForkchoiceState {
                head_block_hash: block1.hash(),
                finalized_block_hash: block1.hash(),
                ..Default::default()
            };

            let result = env.send_forkchoice_updated(forkchoice).await.unwrap();
            let expected_result = ForkchoiceUpdated::new(PayloadStatus::new(
                PayloadStatusEnum::Valid,
                Some(block1.hash()),
            ));
            assert_eq!(result, expected_result);
            assert_matches!(engine_rx.try_recv(), Err(TryRecvError::Empty));
        }

        #[tokio::test]
        async fn unknown_head_hash() {
            let mut rng = generators::rng();

            let chain_spec = Arc::new(
                ChainSpecBuilder::default()
                    .chain(MAINNET.chain)
                    .genesis(MAINNET.genesis.clone())
                    .paris_activated()
                    .build(),
            );

            let (consensus_engine, env) = TestConsensusEngineBuilder::new(chain_spec.clone())
                .with_pipeline_exec_outputs(VecDeque::from([
                    Ok(ExecOutput { checkpoint: StageCheckpoint::new(0), done: true }),
                    Ok(ExecOutput { checkpoint: StageCheckpoint::new(0), done: true }),
                ]))
                .disable_blockchain_tree_sync()
                .build();

            let genesis = random_block(&mut rng, 0, None, None, Some(0));
            let block1 = random_block(&mut rng, 1, Some(genesis.hash()), None, Some(0));

            let (_static_dir, static_dir_path) = create_test_static_files_dir();

            insert_blocks(
                ProviderFactory::new(
                    env.db.as_ref(),
                    chain_spec.clone(),
                    StaticFileProvider::read_write(static_dir_path).unwrap(),
                ),
                [&genesis, &block1].into_iter(),
            );

            let mut engine_rx = spawn_consensus_engine(consensus_engine);

            let next_head = random_block(&mut rng, 2, Some(block1.hash()), None, Some(0));
            let next_forkchoice_state = ForkchoiceState {
                head_block_hash: next_head.hash(),
                finalized_block_hash: block1.hash(),
                ..Default::default()
            };

            // if we `await` in the assert, the forkchoice will poll after we've inserted the block,
            // and it will return VALID instead of SYNCING
            let invalid_rx = env.send_forkchoice_updated(next_forkchoice_state).await;
            let (_static_dir, static_dir_path) = create_test_static_files_dir();

            // Insert next head immediately after sending forkchoice update
            insert_blocks(
                ProviderFactory::new(
                    env.db.as_ref(),
                    chain_spec.clone(),
                    StaticFileProvider::read_write(static_dir_path).unwrap(),
                ),
                std::iter::once(&next_head),
            );

            let expected_result = ForkchoiceUpdated::from_status(PayloadStatusEnum::Syncing);
            assert_matches!(invalid_rx, Ok(result) => assert_eq!(result, expected_result));

            let result = env.send_forkchoice_retry_on_syncing(next_forkchoice_state).await.unwrap();
            let expected_result = ForkchoiceUpdated::from_status(PayloadStatusEnum::Valid)
                .with_latest_valid_hash(next_head.hash());
            assert_eq!(result, expected_result);

            assert_matches!(engine_rx.try_recv(), Err(TryRecvError::Empty));
        }

        #[tokio::test]
        async fn unknown_finalized_hash() {
            let mut rng = generators::rng();
            let chain_spec = Arc::new(
                ChainSpecBuilder::default()
                    .chain(MAINNET.chain)
                    .genesis(MAINNET.genesis.clone())
                    .paris_activated()
                    .build(),
            );

            let (consensus_engine, env) = TestConsensusEngineBuilder::new(chain_spec.clone())
                .with_pipeline_exec_outputs(VecDeque::from([Ok(ExecOutput {
                    checkpoint: StageCheckpoint::new(0),
                    done: true,
                })]))
                .disable_blockchain_tree_sync()
                .build();

            let genesis = random_block(&mut rng, 0, None, None, Some(0));
            let block1 = random_block(&mut rng, 1, Some(genesis.hash()), None, Some(0));

            let (_static_dir, static_dir_path) = create_test_static_files_dir();

            insert_blocks(
                ProviderFactory::new(
                    env.db.as_ref(),
                    chain_spec.clone(),
                    StaticFileProvider::read_write(static_dir_path).unwrap(),
                ),
                [&genesis, &block1].into_iter(),
            );

            let engine = spawn_consensus_engine(consensus_engine);

            let res = env
                .send_forkchoice_updated(ForkchoiceState {
                    head_block_hash: rng.gen(),
                    finalized_block_hash: block1.hash(),
                    ..Default::default()
                })
                .await;
            let expected_result = ForkchoiceUpdated::from_status(PayloadStatusEnum::Syncing);
            assert_matches!(res, Ok(result) => assert_eq!(result, expected_result));
            drop(engine);
        }

        #[tokio::test]
        async fn forkchoice_updated_pre_merge() {
            let mut rng = generators::rng();
            let chain_spec = Arc::new(
                ChainSpecBuilder::default()
                    .chain(MAINNET.chain)
                    .genesis(MAINNET.genesis.clone())
                    .london_activated()
                    .paris_at_ttd(U256::from(3))
                    .build(),
            );

            let (consensus_engine, env) = TestConsensusEngineBuilder::new(chain_spec.clone())
                .with_pipeline_exec_outputs(VecDeque::from([
                    Ok(ExecOutput { checkpoint: StageCheckpoint::new(0), done: true }),
                    Ok(ExecOutput { checkpoint: StageCheckpoint::new(0), done: true }),
                ]))
                .build();

            let genesis = random_block(&mut rng, 0, None, None, Some(0));
            let mut block1 = random_block(&mut rng, 1, Some(genesis.hash()), None, Some(0));
            block1.header.set_difficulty(U256::from(1));

            // a second pre-merge block
            let mut block2 = random_block(&mut rng, 1, Some(genesis.hash()), None, Some(0));
            block2.header.set_difficulty(U256::from(1));

            // a transition block
            let mut block3 = random_block(&mut rng, 1, Some(genesis.hash()), None, Some(0));
            block3.header.set_difficulty(U256::from(1));

            let (_static_dir, static_dir_path) = create_test_static_files_dir();
            insert_blocks(
                ProviderFactory::new(
                    env.db.as_ref(),
                    chain_spec.clone(),
                    StaticFileProvider::read_write(static_dir_path).unwrap(),
                ),
                [&genesis, &block1, &block2, &block3].into_iter(),
            );

            let _engine = spawn_consensus_engine(consensus_engine);

            let res = env
                .send_forkchoice_updated(ForkchoiceState {
                    head_block_hash: block1.hash(),
                    finalized_block_hash: block1.hash(),
                    ..Default::default()
                })
                .await;

            assert_matches!(res, Ok(result) => {
                let ForkchoiceUpdated { payload_status, .. } = result;
                assert_matches!(payload_status.status, PayloadStatusEnum::Invalid { .. });
                assert_eq!(payload_status.latest_valid_hash, Some(B256::ZERO));
            });
        }

        #[tokio::test]
        async fn forkchoice_updated_invalid_pow() {
            let mut rng = generators::rng();
            let chain_spec = Arc::new(
                ChainSpecBuilder::default()
                    .chain(MAINNET.chain)
                    .genesis(MAINNET.genesis.clone())
                    .london_activated()
                    .build(),
            );

            let (consensus_engine, env) = TestConsensusEngineBuilder::new(chain_spec.clone())
                .with_pipeline_exec_outputs(VecDeque::from([
                    Ok(ExecOutput { checkpoint: StageCheckpoint::new(0), done: true }),
                    Ok(ExecOutput { checkpoint: StageCheckpoint::new(0), done: true }),
                ]))
                .build();

            let genesis = random_block(&mut rng, 0, None, None, Some(0));
            let block1 = random_block(&mut rng, 1, Some(genesis.hash()), None, Some(0));

            let (_temp_dir, temp_dir_path) = create_test_static_files_dir();

            insert_blocks(
                ProviderFactory::new(
                    env.db.as_ref(),
                    chain_spec.clone(),
                    StaticFileProvider::read_write(temp_dir_path).unwrap(),
                ),
                [&genesis, &block1].into_iter(),
            );

            let _engine = spawn_consensus_engine(consensus_engine);

            let res = env
                .send_forkchoice_updated(ForkchoiceState {
                    head_block_hash: block1.hash(),
                    finalized_block_hash: block1.hash(),
                    ..Default::default()
                })
                .await;
            let expected_result = ForkchoiceUpdated::from_status(PayloadStatusEnum::Invalid {
                validation_error: BlockValidationError::BlockPreMerge { hash: block1.hash() }
                    .to_string(),
            })
            .with_latest_valid_hash(B256::ZERO);
            assert_matches!(res, Ok(result) => assert_eq!(result, expected_result));
        }
    }

    mod new_payload {
        use super::*;
        use alloy_genesis::Genesis;
        use reth_db::test_utils::create_test_static_files_dir;
        use reth_primitives::{EthereumHardfork, U256};
        use reth_provider::{
            providers::StaticFileProvider, test_utils::blocks::BlockchainTestData,
        };
        use reth_testing_utils::{generators::random_block, GenesisAllocator};
        #[tokio::test]
        async fn new_payload_before_forkchoice() {
            let mut rng = generators::rng();
            let chain_spec = Arc::new(
                ChainSpecBuilder::default()
                    .chain(MAINNET.chain)
                    .genesis(MAINNET.genesis.clone())
                    .paris_activated()
                    .build(),
            );

            let (consensus_engine, env) = TestConsensusEngineBuilder::new(chain_spec.clone())
                .with_pipeline_exec_outputs(VecDeque::from([Ok(ExecOutput {
                    checkpoint: StageCheckpoint::new(0),
                    done: true,
                })]))
                .build();

            let mut engine_rx = spawn_consensus_engine(consensus_engine);

            // Send new payload
            let res = env
                .send_new_payload(
                    block_to_payload_v1(random_block(&mut rng, 0, None, None, Some(0))),
                    None,
                )
                .await;

            // Invalid, because this is a genesis block
            assert_matches!(res, Ok(result) => assert_matches!(result.status, PayloadStatusEnum::Invalid { .. }));

            // Send new payload
            let res = env
                .send_new_payload(
                    block_to_payload_v1(random_block(&mut rng, 1, None, None, Some(0))),
                    None,
                )
                .await;

            let expected_result = PayloadStatus::from_status(PayloadStatusEnum::Syncing);
            assert_matches!(res, Ok(result) => assert_eq!(result, expected_result));

            assert_matches!(engine_rx.try_recv(), Err(TryRecvError::Empty));
        }

        #[tokio::test]
        async fn payload_known() {
            let mut rng = generators::rng();
            let chain_spec = Arc::new(
                ChainSpecBuilder::default()
                    .chain(MAINNET.chain)
                    .genesis(MAINNET.genesis.clone())
                    .paris_activated()
                    .build(),
            );

            let (consensus_engine, env) = TestConsensusEngineBuilder::new(chain_spec.clone())
                .with_pipeline_exec_outputs(VecDeque::from([Ok(ExecOutput {
                    checkpoint: StageCheckpoint::new(0),
                    done: true,
                })]))
                .build();

            let genesis = random_block(&mut rng, 0, None, None, Some(0));
            let block1 = random_block(&mut rng, 1, Some(genesis.hash()), None, Some(0));
            let block2 = random_block(&mut rng, 2, Some(block1.hash()), None, Some(0));

            let (_static_dir, static_dir_path) = create_test_static_files_dir();
            insert_blocks(
                ProviderFactory::new(
                    env.db.as_ref(),
                    chain_spec.clone(),
                    StaticFileProvider::read_write(static_dir_path).unwrap(),
                ),
                [&genesis, &block1, &block2].into_iter(),
            );

            let mut engine_rx = spawn_consensus_engine(consensus_engine);

            // Send forkchoice
            let res = env
                .send_forkchoice_updated(ForkchoiceState {
                    head_block_hash: block1.hash(),
                    finalized_block_hash: block1.hash(),
                    ..Default::default()
                })
                .await;
            let expected_result = PayloadStatus::from_status(PayloadStatusEnum::Valid)
                .with_latest_valid_hash(block1.hash());
            assert_matches!(res, Ok(ForkchoiceUpdated { payload_status, .. }) => assert_eq!(payload_status, expected_result));

            // Send new payload
            let result = env
                .send_new_payload_retry_on_syncing(block_to_payload_v1(block2.clone()), None)
                .await
                .unwrap();

            let expected_result = PayloadStatus::from_status(PayloadStatusEnum::Valid)
                .with_latest_valid_hash(block2.hash());
            assert_eq!(result, expected_result);
            assert_matches!(engine_rx.try_recv(), Err(TryRecvError::Empty));
        }

        #[tokio::test]
        async fn simple_validate_block() {
            let mut rng = generators::rng();
            let amount = U256::from(1000000000000000000u64);
            let mut allocator = GenesisAllocator::default().with_rng(&mut rng);
            for _ in 0..16 {
                // add 16 new accounts
                allocator.new_funded_account(amount);
            }

            let alloc = allocator.build();

            let genesis = Genesis::default().extend_accounts(alloc);

            let chain_spec = Arc::new(
                ChainSpecBuilder::default()
                    .chain(MAINNET.chain)
                    .genesis(genesis)
                    .shanghai_activated()
                    .build(),
            );

            let (consensus_engine, env) = TestConsensusEngineBuilder::new(chain_spec.clone())
                .with_real_pipeline()
                .with_real_executor()
                .with_real_consensus()
                .build();

            let genesis =
                SealedBlock { header: chain_spec.sealed_genesis_header(), ..Default::default() };
            let block1 = random_block(&mut rng, 1, Some(chain_spec.genesis_hash()), None, Some(0));

            // TODO: add transactions that transfer from the alloc accounts, generating the new
            // block tx and state root

            let (_static_dir, static_dir_path) = create_test_static_files_dir();

            insert_blocks(
                ProviderFactory::new(
                    env.db.as_ref(),
                    chain_spec.clone(),
                    StaticFileProvider::read_write(static_dir_path).unwrap(),
                ),
                [&genesis, &block1].into_iter(),
            );

            let mut engine_rx = spawn_consensus_engine(consensus_engine);

            // Send forkchoice
            let res = env
                .send_forkchoice_updated(ForkchoiceState {
                    head_block_hash: block1.hash(),
                    finalized_block_hash: block1.hash(),
                    ..Default::default()
                })
                .await;
            let expected_result = PayloadStatus::from_status(PayloadStatusEnum::Valid)
                .with_latest_valid_hash(block1.hash());
            assert_matches!(res, Ok(ForkchoiceUpdated { payload_status, .. }) => assert_eq!(payload_status, expected_result));
            assert_matches!(engine_rx.try_recv(), Err(TryRecvError::Empty));
        }

        #[tokio::test]
        async fn payload_parent_unknown() {
            let mut rng = generators::rng();
            let chain_spec = Arc::new(
                ChainSpecBuilder::default()
                    .chain(MAINNET.chain)
                    .genesis(MAINNET.genesis.clone())
                    .paris_activated()
                    .build(),
            );

            let (consensus_engine, env) = TestConsensusEngineBuilder::new(chain_spec.clone())
                .with_pipeline_exec_outputs(VecDeque::from([Ok(ExecOutput {
                    checkpoint: StageCheckpoint::new(0),
                    done: true,
                })]))
                .build();

            let genesis = random_block(&mut rng, 0, None, None, Some(0));

            let (_static_dir, static_dir_path) = create_test_static_files_dir();

            insert_blocks(
                ProviderFactory::new(
                    env.db.as_ref(),
                    chain_spec.clone(),
                    StaticFileProvider::read_write(static_dir_path).unwrap(),
                ),
                std::iter::once(&genesis),
            );

            let mut engine_rx = spawn_consensus_engine(consensus_engine);

            // Send forkchoice
            let res = env
                .send_forkchoice_updated(ForkchoiceState {
                    head_block_hash: genesis.hash(),
                    finalized_block_hash: genesis.hash(),
                    ..Default::default()
                })
                .await;
            let expected_result = PayloadStatus::from_status(PayloadStatusEnum::Valid)
                .with_latest_valid_hash(genesis.hash());
            assert_matches!(res, Ok(ForkchoiceUpdated { payload_status, .. }) => assert_eq!(payload_status, expected_result));

            // Send new payload
            let parent = rng.gen();
            let block = random_block(&mut rng, 2, Some(parent), None, Some(0));
            let res = env.send_new_payload(block_to_payload_v1(block), None).await;
            let expected_result = PayloadStatus::from_status(PayloadStatusEnum::Syncing);
            assert_matches!(res, Ok(result) => assert_eq!(result, expected_result));

            assert_matches!(engine_rx.try_recv(), Err(TryRecvError::Empty));
        }

        #[tokio::test]
        async fn payload_pre_merge() {
            let data = BlockchainTestData::default();
            let mut block1 = data.blocks[0].0.block.clone();
            block1.header.set_difficulty(
                MAINNET.fork(EthereumHardfork::Paris).ttd().unwrap() - U256::from(1),
            );
            block1 = block1.unseal().seal_slow();
            let (block2, exec_result2) = data.blocks[1].clone();
            let mut block2 = block2.unseal().block;
            block2.withdrawals = None;
            block2.header.parent_hash = block1.hash();
            block2.header.base_fee_per_gas = Some(100);
            block2.header.difficulty = U256::ZERO;
            let block2 = block2.clone().seal_slow();

            let chain_spec = Arc::new(
                ChainSpecBuilder::default()
                    .chain(MAINNET.chain)
                    .genesis(MAINNET.genesis.clone())
                    .london_activated()
                    .build(),
            );

            let (consensus_engine, env) = TestConsensusEngineBuilder::new(chain_spec.clone())
                .with_pipeline_exec_outputs(VecDeque::from([Ok(ExecOutput {
                    checkpoint: StageCheckpoint::new(0),
                    done: true,
                })]))
                .with_executor_results(Vec::from([exec_result2]))
                .build();

            let (_static_dir, static_dir_path) = create_test_static_files_dir();

            insert_blocks(
                ProviderFactory::new(
                    env.db.as_ref(),
                    chain_spec.clone(),
                    StaticFileProvider::read_write(static_dir_path).unwrap(),
                ),
                [&data.genesis, &block1].into_iter(),
            );

            let mut engine_rx = spawn_consensus_engine(consensus_engine);

            // Send forkchoice
            let res = env
                .send_forkchoice_updated(ForkchoiceState {
                    head_block_hash: block1.hash(),
                    finalized_block_hash: block1.hash(),
                    ..Default::default()
                })
                .await;

            let expected_result = PayloadStatus::from_status(PayloadStatusEnum::Invalid {
                validation_error: BlockValidationError::BlockPreMerge { hash: block1.hash() }
                    .to_string(),
            })
            .with_latest_valid_hash(B256::ZERO);
            assert_matches!(res, Ok(ForkchoiceUpdated { payload_status, .. }) => assert_eq!(payload_status, expected_result));

            // Send new payload
            let result = env
                .send_new_payload_retry_on_syncing(block_to_payload_v1(block2.clone()), None)
                .await
                .unwrap();

            let expected_result = PayloadStatus::from_status(PayloadStatusEnum::Invalid {
                validation_error: BlockValidationError::BlockPreMerge { hash: block2.hash() }
                    .to_string(),
            })
            .with_latest_valid_hash(B256::ZERO);
            assert_eq!(result, expected_result);

            assert_matches!(engine_rx.try_recv(), Err(TryRecvError::Empty));
        }
    }
}

use crate::{
    engine::{message::OnForkChoiceUpdated, metrics::EngineMetrics},
    sync::{EngineSyncController, EngineSyncEvent},
};
use futures::{Future, StreamExt, TryFutureExt};
use reth_db::database::Database;
use reth_interfaces::{
    blockchain_tree::{
        error::{InsertBlockError, InsertBlockErrorKind},
        BlockStatus, BlockchainTreeEngine,
    },
    consensus::ForkchoiceState,
    executor::{BlockExecutionError, BlockValidationError},
    p2p::{bodies::client::BodiesClient, headers::client::HeadersClient},
    sync::{NetworkSyncUpdater, SyncState},
    Error,
};
use reth_payload_builder::{PayloadBuilderAttributes, PayloadBuilderHandle};
use reth_primitives::{
    listener::EventListeners, stage::StageId, BlockNumHash, BlockNumber, Head, Header, SealedBlock,
    SealedHeader, H256, U256,
};
use reth_provider::{
    BlockReader, BlockSource, CanonChainTracker, ProviderError, StageCheckpointReader,
};
use reth_rpc_types::engine::{
    ExecutionPayload, ForkchoiceUpdated, PayloadAttributes, PayloadStatus, PayloadStatusEnum,
    PayloadValidationError,
};
use reth_stages::{ControlFlow, Pipeline, PipelineError};
use reth_tasks::TaskSpawner;
use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::sync::{
    mpsc,
    mpsc::{UnboundedReceiver, UnboundedSender},
    oneshot,
};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::*;

mod message;
pub use message::BeaconEngineMessage;

mod error;
pub use error::{
    BeaconConsensusEngineError, BeaconEngineResult, BeaconForkChoiceUpdateError,
    BeaconOnNewPayloadError,
};

mod invalid_headers;
use invalid_headers::InvalidHeaderCache;
mod metrics;

mod event;
mod forkchoice;
pub(crate) mod sync;

use crate::engine::forkchoice::{ForkchoiceStateHash, ForkchoiceStateTracker};
pub use event::BeaconConsensusEngineEvent;
use reth_interfaces::blockchain_tree::InsertPayloadOk;
use reth_primitives::constants::EPOCH_SLOTS;

/// The maximum number of invalid headers that can be tracked by the engine.
const MAX_INVALID_HEADERS: u32 = 512u32;

/// The largest gap for which the tree will be used for sync. See docs for `pipeline_run_threshold`
/// for more information.
///
/// This is the default threshold, the distance to the head that the tree will be used for sync.
/// If the distance exceeds this threshold, the pipeline will be used for sync.
pub const MIN_BLOCKS_FOR_PIPELINE_RUN: u64 = EPOCH_SLOTS;

/// A _shareable_ beacon consensus frontend. Used to interact with the spawned beacon consensus
/// engine.
///
/// See also [`BeaconConsensusEngine`].
#[derive(Clone, Debug)]
pub struct BeaconConsensusEngineHandle {
    to_engine: UnboundedSender<BeaconEngineMessage>,
}

// === impl BeaconConsensusEngineHandle ===

impl BeaconConsensusEngineHandle {
    /// Creates a new beacon consensus engine handle.
    pub fn new(to_engine: UnboundedSender<BeaconEngineMessage>) -> Self {
        Self { to_engine }
    }

    /// Sends a new payload message to the beacon consensus engine and waits for a response.
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/3d627c95a4d3510a8187dd02e0250ecb4331d27e/src/engine/shanghai.md#engine_newpayloadv2>
    pub async fn new_payload(
        &self,
        payload: ExecutionPayload,
    ) -> Result<PayloadStatus, BeaconOnNewPayloadError> {
        let (tx, rx) = oneshot::channel();
        let _ = self.to_engine.send(BeaconEngineMessage::NewPayload { payload, tx });
        rx.await.map_err(|_| BeaconOnNewPayloadError::EngineUnavailable)?
    }

    /// Sends a forkchoice update message to the beacon consensus engine and waits for a response.
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/3d627c95a4d3510a8187dd02e0250ecb4331d27e/src/engine/shanghai.md#engine_forkchoiceupdatedv2>
    pub async fn fork_choice_updated(
        &self,
        state: ForkchoiceState,
        payload_attrs: Option<PayloadAttributes>,
    ) -> Result<ForkchoiceUpdated, BeaconForkChoiceUpdateError> {
        Ok(self
            .send_fork_choice_updated(state, payload_attrs)
            .map_err(|_| BeaconForkChoiceUpdateError::EngineUnavailable)
            .await??
            .await?)
    }

    /// Sends a forkchoice update message to the beacon consensus engine and returns the receiver to
    /// wait for a response.
    fn send_fork_choice_updated(
        &self,
        state: ForkchoiceState,
        payload_attrs: Option<PayloadAttributes>,
    ) -> oneshot::Receiver<Result<OnForkChoiceUpdated, reth_interfaces::Error>> {
        let (tx, rx) = oneshot::channel();
        let _ = self.to_engine.send(BeaconEngineMessage::ForkchoiceUpdated {
            state,
            payload_attrs,
            tx,
        });
        rx
    }

    /// Sends a transition configuration exchagne message to the beacon consensus engine.
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/3d627c95a4d3510a8187dd02e0250ecb4331d27e/src/engine/paris.md#engine_exchangetransitionconfigurationv1>
    pub async fn transition_configuration_exchanged(&self) {
        let _ = self.to_engine.send(BeaconEngineMessage::TransitionConfigurationExchanged);
    }

    /// Creates a new [`BeaconConsensusEngineEvent`] listener stream.
    pub fn event_listener(&self) -> UnboundedReceiverStream<BeaconConsensusEngineEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        let _ = self.to_engine.send(BeaconEngineMessage::EventListener(tx));
        UnboundedReceiverStream::new(rx)
    }
}

/// The beacon consensus engine is the driver that switches between historical and live sync.
///
/// The beacon consensus engine is itself driven by messages from the Consensus Layer, which are
/// received by Engine API (JSON-RPC).
///
/// The consensus engine is idle until it receives the first
/// [BeaconEngineMessage::ForkchoiceUpdated] message from the CL which would initiate the sync. At
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
/// chain, it will be fully validated added to a chain in the [BlockchainTreeEngine]: `VALID`
///
/// If the payload's chain is disconnected (at least 1 block is missing) then it will be buffered:
/// `SYNCING` ([BlockStatus::Disconnected]).
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
/// All blocks of the `head_hash`'s chain are present in the [BlockchainTreeEngine] and are
/// committed to the canonical chain. This also includes reorgs.
///
/// ### The chain is disconnected
///
/// In this case the [BlockchainTreeEngine] doesn't know how the new chain connects to the existing
/// canonical chain. It could be a simple commit (new blocks extend the current head) or a re-org
/// that requires unwinding the canonical chain.
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
pub struct BeaconConsensusEngine<DB, BT, Client>
where
    DB: Database,
    Client: HeadersClient + BodiesClient,
    BT: BlockchainTreeEngine + BlockReader + CanonChainTracker + StageCheckpointReader,
{
    /// Controls syncing triggered by engine updates.
    sync: EngineSyncController<DB, Client>,
    /// The type we can use to query both the database and the blockchain tree.
    blockchain: BT,
    /// Used for emitting updates about whether the engine is syncing or not.
    sync_state_updater: Box<dyn NetworkSyncUpdater>,
    /// The Engine API message receiver.
    engine_message_rx: UnboundedReceiverStream<BeaconEngineMessage>,
    /// A clone of the handle
    handle: BeaconConsensusEngineHandle,
    /// Tracks the received forkchoice state updates received by the CL.
    forkchoice_state_tracker: ForkchoiceStateTracker,
    /// The payload store.
    payload_builder: PayloadBuilderHandle,
    /// Listeners for engine events.
    listeners: EventListeners<BeaconConsensusEngineEvent>,
    /// Tracks the header of invalid payloads that were rejected by the engine because they're
    /// invalid.
    invalid_headers: InvalidHeaderCache,
    /// Consensus engine metrics.
    metrics: EngineMetrics,
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
}

impl<DB, BT, Client> BeaconConsensusEngine<DB, BT, Client>
where
    DB: Database + Unpin + 'static,
    BT: BlockchainTreeEngine + BlockReader + CanonChainTracker + StageCheckpointReader + 'static,
    Client: HeadersClient + BodiesClient + Clone + Unpin + 'static,
{
    /// Create a new instance of the [BeaconConsensusEngine].
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        client: Client,
        pipeline: Pipeline<DB>,
        blockchain: BT,
        task_spawner: Box<dyn TaskSpawner>,
        sync_state_updater: Box<dyn NetworkSyncUpdater>,
        max_block: Option<BlockNumber>,
        run_pipeline_continuously: bool,
        payload_builder: PayloadBuilderHandle,
        target: Option<H256>,
        pipeline_run_threshold: u64,
    ) -> Result<(Self, BeaconConsensusEngineHandle), reth_interfaces::Error> {
        let (to_engine, rx) = mpsc::unbounded_channel();
        Self::with_channel(
            client,
            pipeline,
            blockchain,
            task_spawner,
            sync_state_updater,
            max_block,
            run_pipeline_continuously,
            payload_builder,
            target,
            pipeline_run_threshold,
            to_engine,
            rx,
        )
    }

    /// Create a new instance of the [BeaconConsensusEngine] using the given channel to configure
    /// the [BeaconEngineMessage] communication channel.
    ///
    /// By default the engine is started with idle pipeline.
    /// The pipeline can be launched immediately in one of the following ways descending in
    /// priority:
    /// - Explicit [Option::Some] target block hash provided via a constructor argument.
    /// - The process was previously interrupted amidst the pipeline run. This is checked by
    ///   comparing the checkpoints of the first ([StageId::Headers]) and last ([StageId::Finish])
    ///   stages. In this case, the latest available header in the database is used as the target.
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
        run_pipeline_continuously: bool,
        payload_builder: PayloadBuilderHandle,
        target: Option<H256>,
        pipeline_run_threshold: u64,
        to_engine: UnboundedSender<BeaconEngineMessage>,
        rx: UnboundedReceiver<BeaconEngineMessage>,
    ) -> Result<(Self, BeaconConsensusEngineHandle), reth_interfaces::Error> {
        let handle = BeaconConsensusEngineHandle { to_engine };
        let sync = EngineSyncController::new(
            pipeline,
            client,
            task_spawner,
            run_pipeline_continuously,
            max_block,
        );
        let mut this = Self {
            sync,
            blockchain,
            sync_state_updater,
            engine_message_rx: UnboundedReceiverStream::new(rx),
            handle: handle.clone(),
            forkchoice_state_tracker: Default::default(),
            payload_builder,
            listeners: EventListeners::default(),
            invalid_headers: InvalidHeaderCache::new(MAX_INVALID_HEADERS),
            metrics: EngineMetrics::default(),
            pipeline_run_threshold,
        };

        let maybe_pipeline_target = match target {
            // Provided target always takes precedence.
            target @ Some(_) => target,
            None => this.check_pipeline_consistency()?,
        };

        if let Some(target) = maybe_pipeline_target {
            this.sync.set_pipeline_sync_target(target);
        }

        Ok((this, handle))
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
    fn check_pipeline_consistency(&self) -> Result<Option<H256>, reth_interfaces::Error> {
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
                return self.blockchain.block_hash(first_stage_checkpoint)
            }
        }

        Ok(None)
    }

    /// Returns a new [`BeaconConsensusEngineHandle`] that can be cloned and shared.
    ///
    /// The [`BeaconConsensusEngineHandle`] can be used to interact with this
    /// [`BeaconConsensusEngine`]
    pub fn handle(&self) -> BeaconConsensusEngineHandle {
        self.handle.clone()
    }

    /// Returns true if the distance from the local tip to the block is greater than the configured
    /// threshold
    fn exceeds_pipeline_run_threshold(&self, local_tip: u64, block: u64) -> bool {
        block > local_tip && block - local_tip > self.pipeline_run_threshold
    }

    /// If validation fails, the response MUST contain the latest valid hash:
    ///
    ///   - The block hash of the ancestor of the invalid payload satisfying the following two
    ///    conditions:
    ///     - It is fully validated and deemed VALID
    ///     - Any other ancestor of the invalid payload with a higher blockNumber is INVALID
    ///   - 0x0000000000000000000000000000000000000000000000000000000000000000 if the above
    ///    conditions are satisfied by a PoW block.
    ///   - null if client software cannot determine the ancestor of the invalid payload satisfying
    ///    the above conditions.
    fn latest_valid_hash_for_invalid_payload(
        &self,
        parent_hash: H256,
        insert_err: Option<&InsertBlockErrorKind>,
    ) -> Option<H256> {
        // check pre merge block error
        if insert_err.map(|err| err.is_block_pre_merge()).unwrap_or_default() {
            return Some(H256::zero())
        }

        // If this is sent from new payload then the parent hash could be in a side chain, and is
        // not necessarily canonical
        if self.blockchain.header_by_hash(parent_hash).is_some() {
            // parent is in side-chain: validated but not canonical yet
            Some(parent_hash)
        } else {
            let parent_hash = self.blockchain.find_canonical_ancestor(parent_hash)?;
            let parent_header = self.blockchain.header(&parent_hash).ok().flatten()?;

            // we need to check if the parent block is the last POW block, if so then the payload is
            // the first POS. The engine API spec mandates a zero hash to be returned: <https://github.com/ethereum/execution-apis/blob/6709c2a795b707202e93c4f2867fa0bf2640a84f/src/engine/paris.md#engine_newpayloadv1>
            if parent_header.difficulty != U256::ZERO {
                return Some(H256::zero())
            }

            // parent is canonical POS block
            Some(parent_hash)
        }
    }

    /// Prepares the invalid payload response for the given hash, checking the
    /// database for the parent hash and populating the payload status with the latest valid hash
    /// according to the engine api spec.
    fn prepare_invalid_response(&self, mut parent_hash: H256) -> PayloadStatus {
        // Edge case: the `latestValid` field is the zero hash if the parent block is the terminal
        // PoW block, which we need to identify by looking at the parent's block difficulty
        if let Ok(Some(parent)) = self.blockchain.header_by_hash_or_number(parent_hash.into()) {
            if parent.difficulty != U256::ZERO {
                parent_hash = H256::zero();
            }
        }

        PayloadStatus::from_status(PayloadStatusEnum::Invalid {
            validation_error: PayloadValidationError::LinksToRejectedPayload.to_string(),
        })
        .with_latest_valid_hash(parent_hash)
    }

    /// Checks if the given `check` hash points to an invalid header, inserting the given `head`
    /// block into the invalid header cache if the `check` hash has a known invalid ancestor.
    ///
    /// Returns a payload status response according to the engine API spec if the block is known to
    /// be invalid.
    fn check_invalid_ancestor_with_head(
        &mut self,
        check: H256,
        head: H256,
    ) -> Option<PayloadStatus> {
        // check if the check hash was previously marked as invalid
        let header = self.invalid_headers.get(&check)?;

        // populate the latest valid hash field
        let status = self.prepare_invalid_response(header.parent_hash);

        // insert the head block into the invalid header cache
        self.invalid_headers.insert_with_invalid_ancestor(head, header);

        Some(status)
    }

    /// Checks if the given `head` points to an invalid header, which requires a specific response
    /// to a forkchoice update.
    fn check_invalid_ancestor(&mut self, head: H256) -> Option<PayloadStatus> {
        let parent_hash = {
            // check if the head was previously marked as invalid
            let header = self.invalid_headers.get(&head)?;
            header.parent_hash
        };

        // populate the latest valid hash field
        let status = self.prepare_invalid_response(parent_hash);

        Some(status)
    }

    /// Invoked when we receive a new forkchoice update message.
    ///
    /// Returns `true` if the engine now reached its maximum block number, See
    /// [EngineSyncController::has_reached_max_block].
    fn on_forkchoice_updated(
        &mut self,
        state: ForkchoiceState,
        attrs: Option<PayloadAttributes>,
        tx: oneshot::Sender<Result<OnForkChoiceUpdated, reth_interfaces::Error>>,
    ) -> bool {
        self.metrics.forkchoice_updated_messages.increment(1);
        self.blockchain.on_forkchoice_update_received(&state);

        let on_updated = match self.forkchoice_updated(state, attrs) {
            Ok(response) => response,
            Err(error) => {
                let _ = tx.send(Err(error));
                return false
            }
        };

        let status = on_updated.forkchoice_status();

        // update the forkchoice state tracker
        self.forkchoice_state_tracker.set_latest(state, status);

        let is_valid_response = on_updated.is_valid_update();
        let _ = tx.send(Ok(on_updated));

        // notify listeners about new processed FCU
        self.listeners.notify(BeaconConsensusEngineEvent::ForkchoiceUpdated(state, status));

        // Terminate the sync early if it's reached the maximum user
        // configured block.
        if is_valid_response {
            // node's fully synced, clear pending requests
            self.sync.clear_full_block_requests();

            let tip_number = self.blockchain.canonical_tip().number;
            if self.sync.has_reached_max_block(tip_number) {
                return true
            }
        }

        false
    }

    /// Called to resolve chain forks and ensure that the Execution layer is working with the latest
    /// valid chain.
    ///
    /// These responses should adhere to the [Engine API Spec for
    /// `engine_forkchoiceUpdated`](https://github.com/ethereum/execution-apis/blob/main/src/engine/paris.md#specification-1).
    ///
    /// Returns an error if an internal error occurred like a database error.
    fn forkchoice_updated(
        &mut self,
        state: ForkchoiceState,
        attrs: Option<PayloadAttributes>,
    ) -> Result<OnForkChoiceUpdated, reth_interfaces::Error> {
        trace!(target: "consensus::engine", ?state, "Received new forkchoice state update");
        if state.head_block_hash.is_zero() {
            return Ok(OnForkChoiceUpdated::invalid_state())
        }

        let lowest_buffered_ancestor_fcu = self.lowest_buffered_ancestor_or(state.head_block_hash);

        if let Some(status) = self.check_invalid_ancestor(lowest_buffered_ancestor_fcu) {
            return Ok(OnForkChoiceUpdated::with_invalid(status))
        }

        if self.sync.is_pipeline_active() {
            // We can only process new forkchoice updates if the pipeline is idle, since it requires
            // exclusive access to the database
            trace!(target: "consensus::engine", "Pipeline is syncing, skipping forkchoice update");
            return Ok(OnForkChoiceUpdated::syncing())
        }

        let status = match self.blockchain.make_canonical(&state.head_block_hash) {
            Ok(outcome) => {
                if !outcome.is_already_canonical() {
                    debug!(target: "consensus::engine", hash=?state.head_block_hash, number=outcome.header().number, "canonicalized new head");

                    // new VALID update that moved the canonical chain forward
                    let _ = self.update_canon_chain(outcome.header().clone(), &state);
                } else {
                    debug!(target: "consensus::engine", fcu_head_num=?outcome.header().number, current_head_num=?self.blockchain.canonical_tip().number, "Ignoring beacon update to old head");
                }

                if let Some(attrs) = attrs {
                    // the CL requested to build a new payload on top of this new VALID head
                    let payload_response = self.process_payload_attributes(
                        attrs,
                        outcome.into_header().unseal(),
                        state,
                    );

                    trace!(target: "consensus::engine", status = ?payload_response, ?state, "Returning forkchoice status");
                    return Ok(payload_response)
                }

                PayloadStatus::new(PayloadStatusEnum::Valid, Some(state.head_block_hash))
            }
            Err(error) => {
                if let Error::Execution(ref err) = error {
                    if err.is_fatal() {
                        tracing::error!(target: "consensus::engine", ?err, "Encountered fatal error");
                        return Err(error)
                    }
                }

                self.on_failed_canonical_forkchoice_update(&state, error)
            }
        };

        trace!(target: "consensus::engine", ?status, ?state, "Returning forkchoice status");
        Ok(OnForkChoiceUpdated::valid(status))
    }

    /// Sets the state of the canon chain tracker based to the given head.
    ///
    /// This expects the given head to be the new canonical head.
    ///
    /// Additionally, updates the head used for p2p handshakes.
    ///
    /// This should be called before issuing a VALID forkchoice update.
    fn update_canon_chain(
        &self,
        head: SealedHeader,
        update: &ForkchoiceState,
    ) -> Result<(), reth_interfaces::Error> {
        let mut head_block = Head {
            number: head.number,
            hash: head.hash,
            difficulty: head.difficulty,
            timestamp: head.timestamp,
            // NOTE: this will be set later
            total_difficulty: Default::default(),
        };

        // we update the the tracked header first
        self.blockchain.set_canonical_head(head);

        if !update.finalized_block_hash.is_zero() {
            let finalized = self
                .blockchain
                .find_block_by_hash(update.finalized_block_hash, BlockSource::Any)?
                .ok_or_else(|| {
                    Error::Provider(ProviderError::UnknownBlockHash(update.finalized_block_hash))
                })?;
            self.blockchain.set_finalized(finalized.header.seal(update.finalized_block_hash));
        }

        if !update.safe_block_hash.is_zero() {
            let safe = self
                .blockchain
                .find_block_by_hash(update.safe_block_hash, BlockSource::Any)?
                .ok_or_else(|| {
                    Error::Provider(ProviderError::UnknownBlockHash(update.safe_block_hash))
                })?;
            self.blockchain.set_safe(safe.header.seal(update.safe_block_hash));
        }

        head_block.total_difficulty =
            self.blockchain.header_td_by_number(head_block.number)?.ok_or_else(|| {
                Error::Provider(ProviderError::TotalDifficultyNotFound {
                    number: head_block.number,
                })
            })?;
        self.sync_state_updater.update_status(head_block);

        Ok(())
    }

    /// Handler for a failed a forkchoice update due to a canonicalization error.
    ///
    /// This will determine if the state's head is invalid, and if so, return immediately.
    ///
    /// If the newest head is not invalid, then this will trigger a new pipeline run to sync the gap
    ///
    /// See [Self::forkchoice_updated] and [BlockchainTreeEngine::make_canonical].
    fn on_failed_canonical_forkchoice_update(
        &mut self,
        state: &ForkchoiceState,
        error: Error,
    ) -> PayloadStatus {
        debug_assert!(self.sync.is_pipeline_idle(), "pipeline must be idle");
        warn!(target: "consensus::engine", ?error, ?state, "Failed to canonicalize the head hash");

        // check if the new head was previously invalidated, if so then we deem this FCU
        // as invalid
        if let Some(invalid_ancestor) = self.check_invalid_ancestor(state.head_block_hash) {
            debug!(target: "consensus::engine", head=?state.head_block_hash, "Head was previously marked as invalid");
            return invalid_ancestor
        }

        #[allow(clippy::single_match)]
        match &error {
            Error::Execution(
                error @ BlockExecutionError::Validation(BlockValidationError::BlockPreMerge {
                    ..
                }),
            ) => {
                return PayloadStatus::from_status(PayloadStatusEnum::Invalid {
                    validation_error: error.to_string(),
                })
                .with_latest_valid_hash(H256::zero())
            }
            _ => {
                // TODO(mattsse) better error handling before attempting to sync (FCU could be
                // invalid): only trigger sync if we can't determine whether the FCU is invalid
            }
        }

        // we assume the FCU is valid and at least the head is missing, so we need to start syncing
        // to it
        let target = if self.forkchoice_state_tracker.is_empty() {
            // find the appropriate target to sync to, if we don't have the safe block hash then we
            // start syncing to the safe block via pipeline first
            let target = if !state.safe_block_hash.is_zero() &&
                self.blockchain.block_number(state.safe_block_hash).ok().flatten().is_none()
            {
                state.safe_block_hash
            } else {
                state.head_block_hash
            };

            // we need to first check the buffer for the head and its ancestors
            let lowest_unknown_hash = self.lowest_buffered_ancestor_or(target);
            trace!(target: "consensus::engine", request=?lowest_unknown_hash, "Triggering full block download for missing ancestors of the new head");
            lowest_unknown_hash
        } else {
            // we need to first check the buffer for the head and its ancestors
            let lowest_unknown_hash = self.lowest_buffered_ancestor_or(state.head_block_hash);
            trace!(target: "consensus::engine", request=?lowest_unknown_hash, "Triggering full block download for missing ancestors of the new head");
            lowest_unknown_hash
        };

        // if the threshold is zero, we should not download the block first, and just use the
        // pipeline. Otherwise we use the tree to insert the block first
        if self.pipeline_run_threshold == 0 {
            // use the pipeline to sync to the target
            self.sync.set_pipeline_sync_target(target);
        } else {
            // trigger a full block download for missing hash, or the parent of its lowest buffered
            // ancestor
            self.sync.download_full_block(target);
        }

        PayloadStatus::from_status(PayloadStatusEnum::Syncing)
    }

    /// Return the parent hash of the lowest buffered ancestor for the requested block, if there
    /// are any buffered ancestors. If there are no buffered ancestors, and the block itself does
    /// not exist in the buffer, this returns the hash that is passed in.
    ///
    /// Returns the parent hash of the block itself if the block is buffered and has no other
    /// buffered ancestors.
    fn lowest_buffered_ancestor_or(&self, hash: H256) -> H256 {
        self.blockchain
            .lowest_buffered_ancestor(hash)
            .map(|block| block.parent_hash)
            .unwrap_or_else(|| hash)
    }

    /// Validates the payload attributes with respect to the header and fork choice state.
    ///
    /// Note: At this point, the fork choice update is considered to be VALID, however, we can still
    /// return an error if the payload attributes are invalid.
    fn process_payload_attributes(
        &self,
        attrs: PayloadAttributes,
        head: Header,
        state: ForkchoiceState,
    ) -> OnForkChoiceUpdated {
        // 7. Client software MUST ensure that payloadAttributes.timestamp is greater than timestamp
        //    of a block referenced by forkchoiceState.headBlockHash. If this condition isn't held
        //    client software MUST respond with -38003: `Invalid payload attributes` and MUST NOT
        //    begin a payload build process. In such an event, the forkchoiceState update MUST NOT
        //    be rolled back.
        if attrs.timestamp <= head.timestamp.into() {
            return OnForkChoiceUpdated::invalid_payload_attributes()
        }

        // 8. Client software MUST begin a payload build process building on top of
        //    forkchoiceState.headBlockHash and identified via buildProcessId value if
        //    payloadAttributes is not null and the forkchoice state has been updated successfully.
        //    The build process is specified in the Payload building section.
        let attributes = PayloadBuilderAttributes::new(state.head_block_hash, attrs);

        // send the payload to the builder and return the receiver for the pending payload id,
        // initiating payload job is handled asynchronously
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
    #[instrument(level = "trace", skip(self, payload), fields(block_hash= ?payload.block_hash, block_number = %payload.block_number.as_u64(), is_pipeline_idle = %self.sync.is_pipeline_idle()), target = "consensus::engine")]
    fn on_new_payload(
        &mut self,
        payload: ExecutionPayload,
    ) -> Result<PayloadStatus, BeaconOnNewPayloadError> {
        let block = match self.ensure_well_formed_payload(payload) {
            Ok(block) => block,
            Err(status) => return Ok(status),
        };
        let block_hash = block.hash();
        let block_num_hash = block.num_hash();

        // now check the block itself
        if let Some(status) = self.check_invalid_ancestor_with_head(block.parent_hash, block.hash) {
            return Ok(status)
        }

        let res = if self.sync.is_pipeline_idle() {
            // we can only insert new payloads if the pipeline is _not_ running, because it holds
            // exclusive access to the database
            self.try_insert_new_payload(block)
        } else {
            self.try_buffer_payload(block)
        };

        let status = match res {
            Ok(status) => {
                if status.is_valid() {
                    if let Some(target) = self.forkchoice_state_tracker.sync_target_state() {
                        // if we're currently syncing and the inserted block is the targeted FCU
                        // head block, we can try to make it canonical.
                        if block_hash == target.head_block_hash {
                            self.try_make_sync_target_canonical(block_num_hash);
                        }
                    }
                    // block was successfully inserted, so we can cancel the full block request, if
                    // any exists
                    self.sync.cancel_full_block_request(block_hash);
                }
                Ok(status)
            }
            Err(error) => {
                warn!(target: "consensus::engine", ?error, "Error while processing payload");
                self.map_insert_error(error)
            }
        };

        trace!(target: "consensus::engine", ?status, "Returning payload status");
        status
    }

    /// Ensures that the given payload does not violate any consensus rules that concern the block's
    /// layout, like:
    ///    - missing or invalid base fee
    ///    - invalid extra data
    ///    - invalid transactions
    fn ensure_well_formed_payload(
        &self,
        payload: ExecutionPayload,
    ) -> Result<SealedBlock, PayloadStatus> {
        let parent_hash = payload.parent_hash;
        let block = match SealedBlock::try_from(payload) {
            Ok(block) => block,
            Err(error) => {
                error!(target: "consensus::engine", ?error, "Invalid payload");

                let mut latest_valid_hash = None;
                if !error.is_block_hash_mismatch() {
                    // Engine-API rule:
                    // > `latestValidHash: null` if the blockHash validation has failed
                    latest_valid_hash =
                        self.latest_valid_hash_for_invalid_payload(parent_hash, None);
                }
                let status = PayloadStatusEnum::from(error);

                return Err(PayloadStatus::new(status, latest_valid_hash))
            }
        };

        Ok(block)
    }

    /// When the pipeline is actively syncing the tree is unable to commit any additional blocks
    /// since the pipeline holds exclusive access to the database.
    ///
    /// In this scenario we buffer the payload in the tree if the payload is valid, once the
    /// pipeline finished syncing the tree is then able to also use the buffered payloads to commit
    /// to a (newer) canonical chain.
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

        let block_hash = block.hash;
        let status = self.blockchain.insert_block_without_senders(block.clone())?;
        let mut latest_valid_hash = None;
        let block = Arc::new(block);
        let status = match status {
            InsertPayloadOk::Inserted(BlockStatus::Valid) => {
                latest_valid_hash = Some(block_hash);
                self.listeners.notify(BeaconConsensusEngineEvent::CanonicalBlockAdded(block));
                PayloadStatusEnum::Valid
            }
            InsertPayloadOk::Inserted(BlockStatus::Accepted) => {
                self.listeners.notify(BeaconConsensusEngineEvent::ForkBlockAdded(block));
                PayloadStatusEnum::Accepted
            }
            InsertPayloadOk::Inserted(BlockStatus::Disconnected { .. }) |
            InsertPayloadOk::AlreadySeen(BlockStatus::Disconnected { .. }) => {
                // check if the block's parent is already marked as invalid
                if let Some(status) =
                    self.check_invalid_ancestor_with_head(block.parent_hash, block.hash)
                {
                    return Ok(status)
                }

                // not known to be invalid, but we don't know anything else
                PayloadStatusEnum::Syncing
            }
            InsertPayloadOk::AlreadySeen(BlockStatus::Valid) => {
                latest_valid_hash = Some(block_hash);
                PayloadStatusEnum::Valid
            }
            InsertPayloadOk::AlreadySeen(BlockStatus::Accepted) => PayloadStatusEnum::Accepted,
        };
        Ok(PayloadStatus::new(status, latest_valid_hash))
    }

    /// Maps the error, that occurred while inserting a payload into the tree to its corresponding
    /// result type.
    ///
    /// If the error was due to an invalid payload, the payload is added to the invalid headers
    /// cache and `Ok` with [PayloadStatusEnum::Invalid] is returned.
    ///
    /// This returns an error if the error was internal and assumed not be related to the payload.
    fn map_insert_error(
        &mut self,
        err: InsertBlockError,
    ) -> Result<PayloadStatus, BeaconOnNewPayloadError> {
        let (block, error) = err.split();

        if error.is_invalid_block() {
            // all of these occurred if the payload is invalid
            let parent_hash = block.parent_hash;

            // keep track of the invalid header
            self.invalid_headers.insert(block.header);

            let latest_valid_hash =
                self.latest_valid_hash_for_invalid_payload(parent_hash, Some(&error));
            let status = PayloadStatusEnum::Invalid { validation_error: error.to_string() };
            Ok(PayloadStatus::new(status, latest_valid_hash))
        } else {
            Err(BeaconOnNewPayloadError::Internal(Box::new(error)))
        }
    }

    /// Attempt to restore the tree with the given block hash.
    ///
    /// This is invoked after a full pipeline to update the tree with the most recent canonical
    /// hashes.
    ///
    /// If the given block is missing from the database, this will return `false`. Otherwise, `true`
    /// is returned: the database contains the hash and the tree was updated.
    fn update_tree_on_finished_pipeline(
        &mut self,
        block_hash: H256,
    ) -> Result<bool, reth_interfaces::Error> {
        let synced_to_finalized = match self.blockchain.block_number(block_hash)? {
            Some(number) => {
                // Attempt to restore the tree.
                self.blockchain.restore_canonical_hashes(number)?;
                true
            }
            None => false,
        };
        Ok(synced_to_finalized)
    }

    /// Invoked if we successfully downloaded a new block from the network.
    ///
    /// This will attempt to insert the block into the tree.
    ///
    /// There are several scenarios:
    ///
    /// ## [BlockStatus::Valid]
    ///
    /// The block is connected to the current canonical head and is valid.
    /// If the engine is still SYNCING, then we can try again to make the chain canonical.
    ///
    /// ## [BlockStatus::Accepted]
    ///
    /// All ancestors are known, but the block is not connected to the current canonical _head_. If
    /// the block is an ancestor of the current forkchoice head, then we can try again to make the
    /// chain canonical, which would trigger a reorg in this case since the new head is therefore
    /// not connected to the current head.
    ///
    /// ## [BlockStatus::Disconnected]
    ///
    /// The block is not connected to the canonical head, and we need to download the missing parent
    /// first.
    ///
    /// ## Insert Error
    ///
    /// If the insertion into the tree failed, then the block was well-formed (valid hash), but its
    /// chain is invalid, which means the FCU that triggered the download is invalid. Here we can
    /// stop because there's nothing to do here and the engine needs to wait for another FCU.
    fn on_downloaded_block(&mut self, block: SealedBlock) {
        let downloaded_num_hash = block.num_hash();
        trace!(target: "consensus::engine", hash=?block.hash, number=%block.number, "Downloaded full block");
        // check if the block's parent is already marked as invalid
        if self.check_invalid_ancestor_with_head(block.parent_hash, block.hash).is_some() {
            // can skip this invalid block
            return
        }

        match self.blockchain.insert_block_without_senders(block) {
            Ok(status) => {
                match status {
                    InsertPayloadOk::Inserted(BlockStatus::Valid) => {
                        // block is connected to the current canonical head and is valid.
                        self.try_make_sync_target_canonical(downloaded_num_hash);
                    }
                    InsertPayloadOk::Inserted(BlockStatus::Accepted) => {
                        // block is connected to the canonical chain, but not the current head
                        self.try_make_sync_target_canonical(downloaded_num_hash);
                    }
                    InsertPayloadOk::Inserted(BlockStatus::Disconnected { missing_parent }) => {
                        // block is not connected to the canonical head, we need to download its
                        // missing branch first
                        self.on_disconnected_block(downloaded_num_hash, missing_parent);
                    }
                    _ => (),
                }
            }
            Err(err) => {
                warn!(target: "consensus::engine", ?err, "Failed to insert downloaded block");
                if err.kind().is_invalid_block() {
                    self.invalid_headers.insert(err.into_block().header);
                }
            }
        }
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
    ) {
        // compare the missing parent with the canonical tip
        let canonical_tip_num = self.blockchain.canonical_tip().number;
        let sync_target_state = self.forkchoice_state_tracker.sync_target_state();

        trace!(target: "consensus::engine", ?downloaded_block, ?missing_parent, tip=?canonical_tip_num, "Handling disconnected block");

        let mut exceeds_pipeline_run_threshold =
            self.exceeds_pipeline_run_threshold(canonical_tip_num, missing_parent.number);

        // check if the downloaded block is the tracked safe block
        if let Some(ref state) = sync_target_state {
            if downloaded_block.hash == state.finalized_block_hash {
                // we downloaded the finalized block
                exceeds_pipeline_run_threshold =
                    self.exceeds_pipeline_run_threshold(canonical_tip_num, downloaded_block.number);
            } else if let Some(buffered_finalized) =
                self.blockchain.buffered_header_by_hash(state.finalized_block_hash)
            {
                // if we have buffered the finalized block, we should check how far
                // we're off
                exceeds_pipeline_run_threshold = self
                    .exceeds_pipeline_run_threshold(canonical_tip_num, buffered_finalized.number);
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
                        warn!(target: "consensus::engine", ?err, "Failed to get finalized block header");
                    }
                    Ok(None) => {
                        // we don't have the block yet and the distance exceeds the allowed
                        // threshold
                        self.sync.set_pipeline_sync_target(state.safe_block_hash);
                        // we can exit early here because the pipeline will take care of this
                        return
                    }
                    Ok(Some(_)) => {
                        // we're fully synced to the finalized block
                        // but we want to continue downloading the missing parent
                    }
                }
            }
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
        self.sync.download_full_block(missing_parent.hash);
    }

    /// Attempt to form a new canonical chain based on the current sync target.
    ///
    /// This is invoked when we successfully downloaded a new block from the network which resulted
    /// in either [BlockStatus::Accepted] or [BlockStatus::Valid].
    ///
    /// Note: This will not succeed if the sync target has changed since the block download request
    /// was issued and the new target is still disconnected and additional missing blocks are
    /// downloaded
    fn try_make_sync_target_canonical(&mut self, inserted: BlockNumHash) {
        if let Some(target) = self.forkchoice_state_tracker.sync_target_state() {
            // optimistically try to make the head of the current FCU target canonical, the sync
            // target might have changed since the block download request was issued
            // (new FCU received)
            match self.blockchain.make_canonical(&target.head_block_hash) {
                Ok(outcome) => {
                    let new_head = outcome.into_header();
                    debug!(target: "consensus::engine", hash=?new_head.hash, number=new_head.number, "canonicalized new head");

                    // we can update the FCU blocks
                    let _ = self.update_canon_chain(new_head, &target);

                    // we're no longer syncing
                    self.sync_state_updater.update_sync_state(SyncState::Idle);

                    // clear any active block requests
                    self.sync.clear_full_block_requests();
                }
                Err(err) => {
                    // if we failed to make the FCU's head canonical, because we don't have that
                    // block yet, then we can try to make the inserted block canonical if we know
                    // it's part of the canonical chain: if it's the safe or the finalized block
                    if matches!(
                        err,
                        reth_interfaces::Error::Execution(
                            BlockExecutionError::BlockHashNotFoundInChain { .. }
                        )
                    ) {
                        // if the inserted block is the currently targeted `finalized` or `safe`
                        // block, we will attempt to make them canonical,
                        // because they are also part of the canonical chain and
                        // their missing block range might already be downloaded (buffered).
                        if let Some(target_hash) = ForkchoiceStateHash::find(&target, inserted.hash)
                            .filter(|h| !h.is_head())
                        {
                            let _ = self.blockchain.make_canonical(target_hash.as_ref());
                        }
                    }
                }
            }
        }
    }

    /// Event handler for events emitted by the [EngineSyncController].
    ///
    /// This returns a result to indicate whether the engine future should resolve (fatal error).
    fn on_sync_event(
        &mut self,
        ev: EngineSyncEvent,
    ) -> Option<Result<(), BeaconConsensusEngineError>> {
        match ev {
            EngineSyncEvent::FetchedFullBlock(block) => {
                self.on_downloaded_block(block);
            }
            EngineSyncEvent::PipelineStarted(target) => {
                trace!(target: "consensus::engine", ?target, continuous = target.is_none(), "Started the pipeline");
                self.metrics.pipeline_runs.increment(1);
                self.sync_state_updater.update_sync_state(SyncState::Syncing);
            }
            EngineSyncEvent::PipelineTaskDropped => {
                error!(target: "consensus::engine", "Failed to receive spawned pipeline");
                return Some(Err(BeaconConsensusEngineError::PipelineChannelClosed))
            }
            EngineSyncEvent::PipelineFinished { result, reached_max_block } => {
                return self.on_pipeline_finished(result, reached_max_block)
            }
        };

        None
    }

    /// Invoked when the pipeline has finished.
    ///
    /// Returns an Option to indicate whether the engine future should resolve:
    ///
    /// Returns a result if:
    ///  - Ok(()) if the pipeline finished successfully
    ///  - Err(..) if the pipeline failed fatally
    ///
    /// Returns None if the pipeline finished successfully and engine should continue.
    fn on_pipeline_finished(
        &mut self,
        result: Result<ControlFlow, PipelineError>,
        reached_max_block: bool,
    ) -> Option<Result<(), BeaconConsensusEngineError>> {
        trace!(target: "consensus::engine", ?result, ?reached_max_block, "Pipeline finished");
        match result {
            Ok(ctrl) => {
                if reached_max_block {
                    // Terminate the sync early if it's reached the maximum user
                    // configured block.
                    return Some(Ok(()))
                }

                if let ControlFlow::Unwind { bad_block, .. } = ctrl {
                    trace!(target: "consensus::engine", hash=?bad_block.hash, "Bad block detected in unwind");

                    // update the `invalid_headers` cache with the new invalid headers
                    self.invalid_headers.insert(bad_block);
                    return None
                }

                // update the canon chain if continuous is enabled
                if self.sync.run_pipeline_continuously() {
                    let max_block = ctrl.progress().unwrap_or_default();
                    let max_header = match self.blockchain.sealed_header(max_block) {
                        Ok(header) => match header {
                            Some(header) => header,
                            None => {
                                return Some(Err(Error::Provider(ProviderError::HeaderNotFound(
                                    max_block.into(),
                                ))
                                .into()))
                            }
                        },
                        Err(error) => {
                            error!(target: "consensus::engine", ?error, "Error getting canonical header for continuous sync");
                            return Some(Err(error.into()))
                        }
                    };
                    self.blockchain.set_canonical_head(max_header);
                }

                let sync_target_state = match self.forkchoice_state_tracker.sync_target_state() {
                    Some(current_state) => current_state,
                    None => {
                        // This is only possible if the node was run with `debug.tip`
                        // argument and without CL.
                        warn!(target: "consensus::engine", "No fork choice state available");
                        return None
                    }
                };

                // Next, we check if we need to schedule another pipeline run or transition
                // to live sync via tree.
                // This can arise if we buffer the forkchoice head, and if the head is an
                // ancestor of an invalid block.
                //
                //  * The forkchoice head could be buffered if it were first sent as a `newPayload`
                //    request.
                //
                // In this case, we won't have the head hash in the database, so we would
                // set the pipeline sync target to a known-invalid head.
                //
                // This is why we check the invalid header cache here.
                let lowest_buffered_ancestor =
                    self.lowest_buffered_ancestor_or(sync_target_state.head_block_hash);

                // this inserts the head if the lowest buffered ancestor is invalid
                if self
                    .check_invalid_ancestor_with_head(
                        lowest_buffered_ancestor,
                        sync_target_state.head_block_hash,
                    )
                    .is_none()
                {
                    // Update the state and hashes of the blockchain tree if possible.
                    match self
                        .update_tree_on_finished_pipeline(sync_target_state.finalized_block_hash)
                    {
                        Ok(synced) => {
                            if synced {
                                // we're consider this synced and transition to live sync
                                self.sync_state_updater.update_sync_state(SyncState::Idle);
                            } else {
                                // We don't have the finalized block in the database, so
                                // we need to run another pipeline.
                                self.sync.set_pipeline_sync_target(
                                    sync_target_state.finalized_block_hash,
                                );
                            }
                        }
                        Err(error) => {
                            error!(target: "consensus::engine", ?error, "Error restoring blockchain tree state");
                            return Some(Err(error.into()))
                        }
                    };
                }
            }
            // Any pipeline error at this point is fatal.
            Err(error) => return Some(Err(error.into())),
        };

        None
    }
}

/// On initialization, the consensus engine will poll the message receiver and return
/// [Poll::Pending] until the first forkchoice update message is received.
///
/// As soon as the consensus engine receives the first forkchoice updated message and updates the
/// local forkchoice state, it will launch the pipeline to sync to the head hash.
/// While the pipeline is syncing, the consensus engine will keep processing messages from the
/// receiver and forwarding them to the blockchain tree.
impl<DB, BT, Client> Future for BeaconConsensusEngine<DB, BT, Client>
where
    DB: Database + Unpin + 'static,
    Client: HeadersClient + BodiesClient + Clone + Unpin + 'static,
    BT: BlockchainTreeEngine
        + BlockReader
        + CanonChainTracker
        + StageCheckpointReader
        + Unpin
        + 'static,
{
    type Output = Result<(), BeaconConsensusEngineError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        // Process all incoming messages from the CL, these can affect the state of the
        // SyncController, hence they are polled first, and they're also time sensitive.
        loop {
            let mut engine_messages_pending = false;

            // handle next engine message
            match this.engine_message_rx.poll_next_unpin(cx) {
                Poll::Ready(Some(msg)) => match msg {
                    BeaconEngineMessage::ForkchoiceUpdated { state, payload_attrs, tx } => {
                        if this.on_forkchoice_updated(state, payload_attrs, tx) {
                            return Poll::Ready(Ok(()))
                        }
                    }
                    BeaconEngineMessage::NewPayload { payload, tx } => {
                        this.metrics.new_payload_messages.increment(1);
                        let res = this.on_new_payload(payload);
                        let _ = tx.send(res);
                    }
                    BeaconEngineMessage::TransitionConfigurationExchanged => {
                        this.blockchain.on_transition_configuration_exchanged();
                    }
                    BeaconEngineMessage::EventListener(tx) => {
                        this.listeners.push_listener(tx);
                    }
                },
                Poll::Ready(None) => {
                    unreachable!("Engine holds the a sender to the message channel")
                }
                Poll::Pending => {
                    // no more CL messages to process
                    engine_messages_pending = true;
                }
            }

            // process sync events if any
            match this.sync.poll(cx) {
                Poll::Ready(sync_event) => {
                    if let Some(res) = this.on_sync_event(sync_event) {
                        return Poll::Ready(res)
                    }
                }
                Poll::Pending => {
                    if engine_messages_pending {
                        // both the sync and the engine message receiver are pending
                        return Poll::Pending
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::error::BeaconForkChoiceUpdateError;
    use assert_matches::assert_matches;
    use reth_blockchain_tree::{
        config::BlockchainTreeConfig, externals::TreeExternals, post_state::PostState,
        BlockchainTree, ShareableBlockchainTree,
    };
    use reth_db::{test_utils::create_test_rw_db, DatabaseEnv};
    use reth_interfaces::{
        sync::NoopSyncStateUpdater,
        test_utils::{NoopFullBlockClient, TestConsensus},
    };
    use reth_payload_builder::test_utils::spawn_test_payload_service;
    use reth_primitives::{stage::StageCheckpoint, ChainSpec, ChainSpecBuilder, H256, MAINNET};
    use reth_provider::{
        providers::BlockchainProvider, test_utils::TestExecutorFactory, BlockWriter,
        ProviderFactory,
    };
    use reth_stages::{test_utils::TestStages, ExecOutput, PipelineError, StageError};
    use reth_tasks::TokioTaskExecutor;
    use std::{collections::VecDeque, sync::Arc, time::Duration};
    use tokio::sync::{
        oneshot::{self, error::TryRecvError},
        watch,
    };

    type TestBeaconConsensusEngine = BeaconConsensusEngine<
        Arc<DatabaseEnv>,
        BlockchainProvider<
            Arc<DatabaseEnv>,
            ShareableBlockchainTree<Arc<DatabaseEnv>, TestConsensus, TestExecutorFactory>,
        >,
        NoopFullBlockClient,
    >;

    struct TestEnv<DB> {
        db: DB,
        // Keep the tip receiver around, so it's not dropped.
        #[allow(dead_code)]
        tip_rx: watch::Receiver<H256>,
        engine_handle: BeaconConsensusEngineHandle,
    }

    impl<DB> TestEnv<DB> {
        fn new(
            db: DB,
            tip_rx: watch::Receiver<H256>,
            engine_handle: BeaconConsensusEngineHandle,
        ) -> Self {
            Self { db, tip_rx, engine_handle }
        }

        async fn send_new_payload(
            &self,
            payload: ExecutionPayload,
        ) -> Result<PayloadStatus, BeaconOnNewPayloadError> {
            self.engine_handle.new_payload(payload).await
        }

        /// Sends the `ExecutionPayload` message to the consensus engine and retries if the engine
        /// is syncing.
        async fn send_new_payload_retry_on_syncing(
            &self,
            payload: ExecutionPayload,
        ) -> Result<PayloadStatus, BeaconOnNewPayloadError> {
            loop {
                let result = self.send_new_payload(payload.clone()).await?;
                if !result.is_syncing() {
                    return Ok(result)
                }
            }
        }

        async fn send_forkchoice_updated(
            &self,
            state: ForkchoiceState,
        ) -> Result<ForkchoiceUpdated, BeaconForkChoiceUpdateError> {
            self.engine_handle.fork_choice_updated(state, None).await
        }

        /// Sends the `ForkchoiceUpdated` message to the consensus engine and retries if the engine
        /// is syncing.
        async fn send_forkchoice_retry_on_syncing(
            &self,
            state: ForkchoiceState,
        ) -> Result<ForkchoiceUpdated, BeaconForkChoiceUpdateError> {
            loop {
                let result = self.engine_handle.fork_choice_updated(state, None).await?;
                if !result.is_syncing() {
                    return Ok(result)
                }
            }
        }
    }

    struct TestConsensusEngineBuilder {
        chain_spec: Arc<ChainSpec>,
        pipeline_exec_outputs: VecDeque<Result<ExecOutput, StageError>>,
        executor_results: Vec<PostState>,
        pipeline_run_threshold: Option<u64>,
        max_block: Option<BlockNumber>,
    }

    impl TestConsensusEngineBuilder {
        /// Create a new `TestConsensusEngineBuilder` with the given `ChainSpec`.
        fn new(chain_spec: Arc<ChainSpec>) -> Self {
            Self {
                chain_spec,
                pipeline_exec_outputs: VecDeque::new(),
                executor_results: Vec::new(),
                pipeline_run_threshold: None,
                max_block: None,
            }
        }

        /// Set the pipeline execution outputs to use for the test consensus engine.
        fn with_pipeline_exec_outputs(
            mut self,
            pipeline_exec_outputs: VecDeque<Result<ExecOutput, StageError>>,
        ) -> Self {
            self.pipeline_exec_outputs = pipeline_exec_outputs;
            self
        }

        /// Set the executor results to use for the test consensus engine.
        fn with_executor_results(mut self, executor_results: Vec<PostState>) -> Self {
            self.executor_results = executor_results;
            self
        }

        /// Sets the max block for the pipeline to run.
        fn with_max_block(mut self, max_block: BlockNumber) -> Self {
            self.max_block = Some(max_block);
            self
        }

        /// Disables blockchain tree driven sync. This is the same as setting the pipeline run
        /// threshold to 0.
        fn disable_blockchain_tree_sync(mut self) -> Self {
            self.pipeline_run_threshold = Some(0);
            self
        }

        /// Builds the test consensus engine into a `TestConsensusEngine` and `TestEnv`.
        fn build(self) -> (TestBeaconConsensusEngine, TestEnv<Arc<DatabaseEnv>>) {
            reth_tracing::init_test_tracing();
            let db = create_test_rw_db();
            let consensus = TestConsensus::default();
            let payload_builder = spawn_test_payload_service();

            let executor_factory = TestExecutorFactory::new(self.chain_spec.clone());
            executor_factory.extend(self.executor_results);

            // Setup pipeline
            let (tip_tx, tip_rx) = watch::channel(H256::default());
            let mut pipeline = Pipeline::builder()
                .add_stages(TestStages::new(self.pipeline_exec_outputs, Default::default()))
                .with_tip_sender(tip_tx);

            if let Some(max_block) = self.max_block {
                pipeline = pipeline.with_max_block(max_block);
            }

            let pipeline = pipeline.build(db.clone(), self.chain_spec.clone());

            // Setup blockchain tree
            let externals = TreeExternals::new(
                db.clone(),
                consensus,
                executor_factory,
                self.chain_spec.clone(),
            );
            let config = BlockchainTreeConfig::new(1, 2, 3, 2);
            let (canon_state_notification_sender, _) = tokio::sync::broadcast::channel(3);
            let tree = ShareableBlockchainTree::new(
                BlockchainTree::new(externals, canon_state_notification_sender, config)
                    .expect("failed to create tree"),
            );
            let shareable_db = ProviderFactory::new(db.clone(), self.chain_spec.clone());
            let latest = self.chain_spec.genesis_header().seal_slow();
            let blockchain_provider = BlockchainProvider::with_latest(shareable_db, tree, latest);
            let (mut engine, handle) = BeaconConsensusEngine::new(
                NoopFullBlockClient::default(),
                pipeline,
                blockchain_provider,
                Box::<TokioTaskExecutor>::default(),
                Box::<NoopSyncStateUpdater>::default(),
                None,
                false,
                payload_builder,
                None,
                self.pipeline_run_threshold.unwrap_or(MIN_BLOCKS_FOR_PIPELINE_RUN),
            )
            .expect("failed to create consensus engine");

            if let Some(max_block) = self.max_block {
                engine.sync.set_max_block(max_block)
            }

            (engine, TestEnv::new(db, tip_rx, handle))
        }
    }

    fn spawn_consensus_engine(
        engine: TestBeaconConsensusEngine,
    ) -> oneshot::Receiver<Result<(), BeaconConsensusEngineError>> {
        let (tx, rx) = oneshot::channel();
        tokio::spawn(async move {
            let result = engine.await;
            tx.send(result).expect("failed to forward consensus engine result");
        });
        rx
    }

    // Pipeline error is propagated.
    #[tokio::test]
    async fn pipeline_error_is_propagated() {
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
                head_block_hash: H256::random(),
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
        std::thread::sleep(Duration::from_millis(100));
        assert_matches!(rx.try_recv(), Err(TryRecvError::Empty));

        // consensus engine is still idle
        let _ = env.send_new_payload(SealedBlock::default().into()).await;
        assert_matches!(rx.try_recv(), Err(TryRecvError::Empty));

        // consensus engine receives a forkchoice state and triggers the pipeline
        let _ = env
            .send_forkchoice_updated(ForkchoiceState {
                head_block_hash: H256::random(),
                ..Default::default()
            })
            .await;
        assert_matches!(
            rx.await,
            Ok(Err(BeaconConsensusEngineError::Pipeline(n))) if matches!(*n.as_ref(),PipelineError::Stage(StageError::ChannelClosed))
        );
    }

    // Test that the consensus engine runs the pipeline again if the tree cannot be restored.
    // The consensus engine will propagate the second result (error) only if it runs the pipeline
    // for the second time.
    #[tokio::test]
    async fn runs_pipeline_again_if_tree_not_restored() {
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
                head_block_hash: H256::random(),
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
                head_block_hash: H256::random(),
                ..Default::default()
            })
            .await;
        assert_matches!(rx.await, Ok(Ok(())));
    }

    fn insert_blocks<'a, DB: Database>(
        db: &DB,
        chain: Arc<ChainSpec>,
        mut blocks: impl Iterator<Item = &'a SealedBlock>,
    ) {
        let factory = ProviderFactory::new(db, chain);
        let provider = factory.provider_rw().unwrap();
        blocks
            .try_for_each(|b| provider.insert_block(b.clone(), None).map(|_| ()))
            .expect("failed to insert");
        provider.commit().unwrap();
    }

    mod fork_choice_updated {
        use super::*;
        use reth_db::{tables, transaction::DbTxMut};
        use reth_interfaces::test_utils::{generators, generators::random_block};
        use reth_rpc_types::engine::ForkchoiceUpdateError;

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
            let block1 = random_block(&mut rng, 1, Some(genesis.hash), None, Some(0));
            insert_blocks(env.db.as_ref(), chain_spec.clone(), [&genesis, &block1].into_iter());
            env.db
                .update(|tx| {
                    tx.put::<tables::SyncStage>(
                        StageId::Finish.to_string(),
                        StageCheckpoint::new(block1.number),
                    )
                })
                .unwrap()
                .unwrap();

            let mut engine_rx = spawn_consensus_engine(consensus_engine);

            let forkchoice = ForkchoiceState {
                head_block_hash: block1.hash,
                finalized_block_hash: block1.hash,
                ..Default::default()
            };

            let result = env.send_forkchoice_updated(forkchoice).await.unwrap();
            let expected_result = ForkchoiceUpdated::new(PayloadStatus::new(
                PayloadStatusEnum::Valid,
                Some(block1.hash),
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
            let block1 = random_block(&mut rng, 1, Some(genesis.hash), None, Some(0));
            insert_blocks(env.db.as_ref(), chain_spec.clone(), [&genesis, &block1].into_iter());

            let mut engine_rx = spawn_consensus_engine(consensus_engine);

            let next_head = random_block(&mut rng, 2, Some(block1.hash), None, Some(0));
            let next_forkchoice_state = ForkchoiceState {
                head_block_hash: next_head.hash,
                finalized_block_hash: block1.hash,
                ..Default::default()
            };

            // if we `await` in the assert, the forkchoice will poll after we've inserted the block,
            // and it will return VALID instead of SYNCING
            let invalid_rx = env.send_forkchoice_updated(next_forkchoice_state).await;

            // Insert next head immediately after sending forkchoice update
            insert_blocks(env.db.as_ref(), chain_spec.clone(), [&next_head].into_iter());

            let expected_result = ForkchoiceUpdated::from_status(PayloadStatusEnum::Syncing);
            assert_matches!(invalid_rx, Ok(result) => assert_eq!(result, expected_result));

            let result = env.send_forkchoice_retry_on_syncing(next_forkchoice_state).await.unwrap();
            let expected_result = ForkchoiceUpdated::from_status(PayloadStatusEnum::Valid)
                .with_latest_valid_hash(next_head.hash);
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
            let block1 = random_block(&mut rng, 1, Some(genesis.hash), None, Some(0));
            insert_blocks(env.db.as_ref(), chain_spec.clone(), [&genesis, &block1].into_iter());

            let engine = spawn_consensus_engine(consensus_engine);

            let res = env
                .send_forkchoice_updated(ForkchoiceState {
                    head_block_hash: H256::random(),
                    finalized_block_hash: block1.hash,
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
            let mut block1 = random_block(&mut rng, 1, Some(genesis.hash), None, Some(0));
            block1.header.difficulty = U256::from(1);

            // a second pre-merge block
            let mut block2 = random_block(&mut rng, 1, Some(genesis.hash), None, Some(0));
            block2.header.difficulty = U256::from(1);

            // a transition block
            let mut block3 = random_block(&mut rng, 1, Some(genesis.hash), None, Some(0));
            block3.header.difficulty = U256::from(1);

            insert_blocks(
                env.db.as_ref(),
                chain_spec.clone(),
                [&genesis, &block1, &block2, &block3].into_iter(),
            );

            let _engine = spawn_consensus_engine(consensus_engine);

            let res = env
                .send_forkchoice_updated(ForkchoiceState {
                    head_block_hash: block1.hash,
                    finalized_block_hash: block1.hash,
                    ..Default::default()
                })
                .await;

            assert_matches!(res, Ok(result) => {
                let ForkchoiceUpdated { payload_status, .. } = result;
                assert_matches!(payload_status.status, PayloadStatusEnum::Invalid { .. });
                assert_eq!(payload_status.latest_valid_hash, Some(H256::zero()));
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
            let block1 = random_block(&mut rng, 1, Some(genesis.hash), None, Some(0));

            insert_blocks(env.db.as_ref(), chain_spec.clone(), [&genesis, &block1].into_iter());

            let _engine = spawn_consensus_engine(consensus_engine);

            let res = env
                .send_forkchoice_updated(ForkchoiceState {
                    head_block_hash: block1.hash,
                    finalized_block_hash: block1.hash,
                    ..Default::default()
                })
                .await;
            let expected_result = ForkchoiceUpdated::from_status(PayloadStatusEnum::Invalid {
                validation_error: BlockValidationError::BlockPreMerge { hash: block1.hash }
                    .to_string(),
            })
            .with_latest_valid_hash(H256::zero());
            assert_matches!(res, Ok(result) => assert_eq!(result, expected_result));
        }
    }

    mod new_payload {
        use super::*;
        use reth_interfaces::test_utils::{generators, generators::random_block};
        use reth_primitives::{Hardfork, U256};
        use reth_provider::test_utils::blocks::BlockChainTestData;

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
            let res =
                env.send_new_payload(random_block(&mut rng, 0, None, None, Some(0)).into()).await;
            // Invalid, because this is a genesis block
            assert_matches!(res, Ok(result) => assert_matches!(result.status, PayloadStatusEnum::Invalid { .. }));

            // Send new payload
            let res =
                env.send_new_payload(random_block(&mut rng, 1, None, None, Some(0)).into()).await;
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
            let block1 = random_block(&mut rng, 1, Some(genesis.hash), None, Some(0));
            let block2 = random_block(&mut rng, 2, Some(block1.hash), None, Some(0));
            insert_blocks(
                env.db.as_ref(),
                chain_spec.clone(),
                [&genesis, &block1, &block2].into_iter(),
            );

            let mut engine_rx = spawn_consensus_engine(consensus_engine);

            // Send forkchoice
            let res = env
                .send_forkchoice_updated(ForkchoiceState {
                    head_block_hash: block1.hash,
                    finalized_block_hash: block1.hash,
                    ..Default::default()
                })
                .await;
            let expected_result = PayloadStatus::from_status(PayloadStatusEnum::Valid)
                .with_latest_valid_hash(block1.hash);
            assert_matches!(res, Ok(ForkchoiceUpdated { payload_status, .. }) => assert_eq!(payload_status, expected_result));

            // Send new payload
            let result =
                env.send_new_payload_retry_on_syncing(block2.clone().into()).await.unwrap();
            let expected_result = PayloadStatus::from_status(PayloadStatusEnum::Valid)
                .with_latest_valid_hash(block2.hash);
            assert_eq!(result, expected_result);
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

            insert_blocks(env.db.as_ref(), chain_spec.clone(), [&genesis].into_iter());

            let mut engine_rx = spawn_consensus_engine(consensus_engine);

            // Send forkchoice
            let res = env
                .send_forkchoice_updated(ForkchoiceState {
                    head_block_hash: genesis.hash,
                    finalized_block_hash: genesis.hash,
                    ..Default::default()
                })
                .await;
            let expected_result = PayloadStatus::from_status(PayloadStatusEnum::Valid)
                .with_latest_valid_hash(genesis.hash);
            assert_matches!(res, Ok(ForkchoiceUpdated { payload_status, .. }) => assert_eq!(payload_status, expected_result));

            // Send new payload
            let block = random_block(&mut rng, 2, Some(H256::random()), None, Some(0));
            let res = env.send_new_payload(block.into()).await;
            let expected_result = PayloadStatus::from_status(PayloadStatusEnum::Syncing);
            assert_matches!(res, Ok(result) => assert_eq!(result, expected_result));

            assert_matches!(engine_rx.try_recv(), Err(TryRecvError::Empty));
        }

        #[tokio::test]
        async fn payload_pre_merge() {
            let data = BlockChainTestData::default();
            let mut block1 = data.blocks[0].0.block.clone();
            block1.header.difficulty = MAINNET.fork(Hardfork::Paris).ttd().unwrap() - U256::from(1);
            block1 = block1.unseal().seal_slow();
            let (block2, exec_result2) = data.blocks[1].clone();
            let mut block2 = block2.block;
            block2.withdrawals = None;
            block2.header.parent_hash = block1.hash;
            block2.header.base_fee_per_gas = Some(100);
            block2.header.difficulty = U256::ZERO;
            block2 = block2.unseal().seal_slow();

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

            insert_blocks(
                env.db.as_ref(),
                chain_spec.clone(),
                [&data.genesis, &block1].into_iter(),
            );

            let mut engine_rx = spawn_consensus_engine(consensus_engine);

            // Send forkchoice
            let res = env
                .send_forkchoice_updated(ForkchoiceState {
                    head_block_hash: block1.hash,
                    finalized_block_hash: block1.hash,
                    ..Default::default()
                })
                .await;

            let expected_result = PayloadStatus::from_status(PayloadStatusEnum::Invalid {
                validation_error: BlockValidationError::BlockPreMerge { hash: block1.hash }
                    .to_string(),
            })
            .with_latest_valid_hash(H256::zero());
            assert_matches!(res, Ok(ForkchoiceUpdated { payload_status, .. }) => assert_eq!(payload_status, expected_result));

            // Send new payload
            let result =
                env.send_new_payload_retry_on_syncing(block2.clone().into()).await.unwrap();

            let expected_result = PayloadStatus::from_status(PayloadStatusEnum::Invalid {
                validation_error: BlockValidationError::BlockPreMerge { hash: block2.hash }
                    .to_string(),
            })
            .with_latest_valid_hash(H256::zero());
            assert_eq!(result, expected_result);

            assert_matches!(engine_rx.try_recv(), Err(TryRecvError::Empty));
        }
    }
}

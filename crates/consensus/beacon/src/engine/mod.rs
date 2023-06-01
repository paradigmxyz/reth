use crate::{
    engine::{message::OnForkChoiceUpdated, metrics::Metrics},
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
    executor::BlockExecutionError,
    p2p::{bodies::client::BodiesClient, headers::client::HeadersClient},
    sync::{NetworkSyncUpdater, SyncState},
    Error,
};
use reth_payload_builder::{PayloadBuilderAttributes, PayloadBuilderHandle};
use reth_primitives::{
    listener::EventListeners, stage::StageId, BlockNumber, Head, Header, SealedBlock, SealedHeader,
    H256, U256,
};
use reth_provider::{
    BlockProvider, BlockSource, CanonChainTracker, ProviderError, StageCheckpointProvider,
};
use reth_rpc_types::engine::{
    ExecutionPayload, ForkchoiceUpdated, PayloadAttributes, PayloadStatus, PayloadStatusEnum,
    PayloadValidationError,
};
use reth_stages::{ControlFlow, Pipeline};
use reth_tasks::TaskSpawner;
use schnellru::{ByLength, LruMap};
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

mod metrics;

mod event;
mod forkchoice;
pub(crate) mod sync;

use crate::engine::forkchoice::ForkchoiceStateTracker;
pub use event::BeaconConsensusEngineEvent;

/// The maximum number of invalid headers that can be tracked by the engine.
const MAX_INVALID_HEADERS: u32 = 512u32;

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
    /// See also <https://github.com/ethereum/execution-apis/blob/8db51dcd2f4bdfbd9ad6e4a7560aac97010ad063/src/engine/specification.md#engine_newpayloadv2>
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
    /// See also <https://github.com/ethereum/execution-apis/blob/main/src/engine/specification.md#engine_forkchoiceupdatedv2>
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
/// received by Engine API.
///
/// The consensus engine is idle until it receives the first
/// [BeaconEngineMessage::ForkchoiceUpdated] message from the CL which would initiate the sync. At
/// first, the consensus engine would run the [Pipeline] until the latest known block hash.
/// Afterwards, it would attempt to create/restore the [`BlockchainTreeEngine`] from the blocks
/// that are currently available. In case the restoration is successful, the consensus engine would
/// run in a live sync mode, which mean it would solemnly rely on the messages from Engine API to
/// construct the chain forward.
///
/// # Panics
///
/// If the future is polled more than once. Leads to undefined state.
#[must_use = "Future does nothing unless polled"]
pub struct BeaconConsensusEngine<DB, BT, Client>
where
    DB: Database,
    Client: HeadersClient + BodiesClient,
    BT: BlockchainTreeEngine + BlockProvider + CanonChainTracker + StageCheckpointProvider,
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
    metrics: Metrics,
}

impl<DB, BT, Client> BeaconConsensusEngine<DB, BT, Client>
where
    DB: Database + Unpin + 'static,
    BT: BlockchainTreeEngine
        + BlockProvider
        + CanonChainTracker
        + StageCheckpointProvider
        + 'static,
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
            metrics: Metrics::default(),
        };

        let maybe_pipeline_target = match target {
            // Provided target always takes precedence.
            target @ Some(_) => target,
            None => {
                // If no target was provided, check if the stages are congruent - check if the
                // checkpoint of the last stage matches the checkpoint of the first.
                let first_stage_checkpoint = this
                    .blockchain
                    .get_stage_checkpoint(*StageId::ALL.first().unwrap())?
                    .unwrap_or_default()
                    .block_number;
                let last_stage_checkpoint = this
                    .blockchain
                    .get_stage_checkpoint(*StageId::ALL.last().unwrap())?
                    .unwrap_or_default()
                    .block_number;
                if first_stage_checkpoint != last_stage_checkpoint {
                    this.blockchain.block_hash(first_stage_checkpoint)?
                } else {
                    None
                }
            }
        };
        if let Some(target) = maybe_pipeline_target {
            this.sync.set_pipeline_sync_target(target);
        }

        Ok((this, handle))
    }

    /// Returns a new [`BeaconConsensusEngineHandle`] that can be cloned and shared.
    ///
    /// The [`BeaconConsensusEngineHandle`] can be used to interact with this
    /// [`BeaconConsensusEngine`]
    pub fn handle(&self) -> BeaconConsensusEngineHandle {
        self.handle.clone()
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
        let header = { self.invalid_headers.get(&check)?.clone() };

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

        // popualte the latest valid hash field
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

        let on_updated = match self.forkchoice_updated(state, attrs) {
            Ok(response) => response,
            Err(error) => {
                let _ = tx.send(Err(error));
                return false
            }
        };

        // update the forkchoice state tracker
        self.forkchoice_state_tracker.set_latest(state, on_updated.forkchoice_status());

        let is_valid_response = on_updated.is_valid_update();
        let _ = tx.send(Ok(on_updated));

        // Terminate the sync early if it's reached the maximum user
        // configured block.
        if is_valid_response {
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

        // TODO: check PoW / EIP-3675 terminal block conditions for the fork choice head
        // TODO: ensure validity of the payload (is this satisfied already?)

        let status = if self.sync.is_pipeline_idle() {
            // We can only process new forkchoice updates if the pipeline is idle, since it requires
            // exclusive access to the database
            match self.blockchain.make_canonical(&state.head_block_hash) {
                Ok(outcome) => {
                    let header = outcome.into_header();
                    debug!(target: "consensus::engine", hash=?state.head_block_hash, number=header.number, "canonicalized new head");

                    let pipeline_min_progress = self
                        .blockchain
                        .get_stage_checkpoint(StageId::Finish)?
                        .unwrap_or_default()
                        .block_number;

                    if pipeline_min_progress < header.number {
                        debug!(target: "consensus::engine", last_finished=pipeline_min_progress, head_number=header.number, "pipeline run to head required");

                        // TODO(mattsse) ideally sync blockwise
                        self.sync.set_pipeline_sync_target(state.head_block_hash);
                    }

                    if let Some(attrs) = attrs {
                        let payload_response =
                            self.process_payload_attributes(attrs, header.unseal(), state);
                        if payload_response.is_valid_update() {
                            // we will return VALID, so let's make sure the info tracker is
                            // properly updated
                            self.update_canon_chain(&state)?;
                        }
                        self.listeners.notify(BeaconConsensusEngineEvent::ForkchoiceUpdated(state));
                        trace!(target: "consensus::engine", status = ?payload_response, ?state, "Returning forkchoice status ");
                        return Ok(payload_response)
                    }

                    // we will return VALID, so let's make sure the info tracker is
                    // properly updated
                    self.update_canon_chain(&state)?;
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
            }
        } else {
            trace!(target: "consensus::engine", "Pipeline is syncing, skipping forkchoice update");
            PayloadStatus::from_status(PayloadStatusEnum::Syncing)
        };

        self.listeners.notify(BeaconConsensusEngineEvent::ForkchoiceUpdated(state));

        trace!(target: "consensus::engine", ?status, ?state, "Returning forkchoice status");
        Ok(OnForkChoiceUpdated::valid(status))
    }

    /// Sets the state of the canon chain tracker based on the given forkchoice update.
    /// Additionally, updates the head used for p2p handshakes.
    ///
    /// This should be called before issuing a VALID forkchoice update.
    fn update_canon_chain(&self, update: &ForkchoiceState) -> Result<(), reth_interfaces::Error> {
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

        // the consensus engine should ensure the head is not zero so we always update the head
        let head = self
            .blockchain
            .find_block_by_hash(update.head_block_hash, BlockSource::Any)?
            .ok_or_else(|| {
                Error::Provider(ProviderError::UnknownBlockHash(update.head_block_hash))
            })?;
        let head = head.header.seal(update.head_block_hash);
        let head_td = self.blockchain.header_td_by_number(head.number)?.ok_or_else(|| {
            Error::Provider(ProviderError::TotalDifficultyNotFound { number: head.number })
        })?;

        self.sync_state_updater.update_status(Head {
            number: head.number,
            hash: head.hash,
            difficulty: head.difficulty,
            timestamp: head.timestamp,
            total_difficulty: head_td,
        });
        self.blockchain.set_canonical_head(head);
        self.blockchain.on_forkchoice_update_received(update);
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
        warn!(target: "consensus::engine", ?error, ?state, "Error canonicalizing the head hash");

        // check if the new head was previously invalidated, if so then we deem this FCU
        // as invalid
        if let Some(invalid_ancestor) = self.check_invalid_ancestor(state.head_block_hash) {
            debug!(target: "consensus::engine", head=?state.head_block_hash, "Head was previously marked as invalid");
            return invalid_ancestor
        }

        #[allow(clippy::single_match)]
        match &error {
            Error::Execution(error @ BlockExecutionError::BlockPreMerge { .. }) => {
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

        // if this is the first FCU we received from the beacon node, then we start triggering the
        // pipeline
        if self.forkchoice_state_tracker.is_empty() {
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

            trace!(target: "consensus::engine", request=?lowest_unknown_hash, "Triggering pipeline with target instead of downloading");

            self.sync.set_pipeline_sync_target(lowest_unknown_hash);
        } else {
            // we need to first check the buffer for the head and its ancestors
            let lowest_unknown_hash = self.lowest_buffered_ancestor_or(state.head_block_hash);

            trace!(target: "consensus::engine", request=?lowest_unknown_hash, "Triggering full block download for missing ancestors of the new head");

            // trigger a full block download for missing hash, or the parent of its lowest buffered
            // ancestor
            self.sync.download_full_block(lowest_unknown_hash);
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
        // 7. Client software MUST ensure that payloadAttributes.timestamp is
        //    greater than timestamp of a block referenced by
        //    forkchoiceState.headBlockHash. If this condition isn't held client
        //    software MUST respond with -38003: `Invalid payload attributes` and
        //    MUST NOT begin a payload build process. In such an event, the
        //    forkchoiceState update MUST NOT be rolled back.
        if attrs.timestamp <= head.timestamp.into() {
            return OnForkChoiceUpdated::invalid_payload_attributes()
        }

        // 8. Client software MUST begin a payload build process building on top of
        //    forkchoiceState.headBlockHash and identified via buildProcessId value
        //    if payloadAttributes is not null and the forkchoice state has been
        //    updated successfully. The build process is specified in the Payload
        //    building section.
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

        // now check the block itself
        if let Some(status) = self.check_invalid_ancestor(block.parent_hash) {
            // The parent is invalid, so this block is also invalid
            self.invalid_headers.insert(block.header);
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
                    // block was successfully inserted, so we can cancel the full block request, if
                    // any exists
                    self.sync.cancel_full_block_request(block_hash);
                }
                Ok(status)
            }
            Err(error) => {
                debug!(target: "consensus::engine", ?error, "Error while processing payload");
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
            BlockStatus::Valid => {
                latest_valid_hash = Some(block_hash);
                self.listeners.notify(BeaconConsensusEngineEvent::CanonicalBlockAdded(block));
                PayloadStatusEnum::Valid
            }
            BlockStatus::Accepted => {
                self.listeners.notify(BeaconConsensusEngineEvent::ForkBlockAdded(block));
                PayloadStatusEnum::Accepted
            }
            BlockStatus::Disconnected { .. } => {
                // check if the block's parent is already marked as invalid
                if let Some(status) =
                    self.check_invalid_ancestor_with_head(block.parent_hash, block.hash)
                {
                    return Ok(status)
                }

                // not known to be invalid, but we don't know anything else
                PayloadStatusEnum::Syncing
            }
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
        match error {
            InsertBlockErrorKind::Internal(err) => {
                // this is an internal error that is unrelated to the payload
                Err(BeaconOnNewPayloadError::Internal(err))
            }
            InsertBlockErrorKind::SenderRecovery |
            InsertBlockErrorKind::Consensus(_) |
            InsertBlockErrorKind::Execution(_) |
            InsertBlockErrorKind::Tree(_) => {
                // all of these occurred if the payload is invalid
                let parent_hash = block.parent_hash;

                // keep track of the invalid header
                self.invalid_headers.insert(block.header);

                let latest_valid_hash =
                    self.latest_valid_hash_for_invalid_payload(parent_hash, Some(&error));
                let status = PayloadStatusEnum::Invalid { validation_error: error.to_string() };
                Ok(PayloadStatus::new(status, latest_valid_hash))
            }
        }
    }

    /// Attempt to restore the tree with the finalized block number.
    /// If the finalized block is missing from the database, trigger the pipeline run.
    fn restore_tree_if_possible(
        &mut self,
        state: ForkchoiceState,
    ) -> Result<(), reth_interfaces::Error> {
        let needs_pipeline_run = match self.blockchain.block_number(state.finalized_block_hash)? {
            Some(number) => {
                // Attempt to restore the tree.
                self.blockchain.restore_canonical_hashes(number)?;

                // After restoring the tree, check if the head block is missing.
                self.blockchain.header(&state.head_block_hash)?.is_none()
            }
            None => true,
        };

        if needs_pipeline_run {
            self.sync.set_pipeline_sync_target(state.head_block_hash);
        }
        Ok(())
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
                trace!(target: "consensus::engine", hash=?block.hash, "Fetched full block");
                // it is guaranteed that the pipeline is not active at this point.

                // TODO(mattsse): better error handling and start closing the gap if there's any by
                //  closing the gap either via pipeline, or by fetching the blocks via block number
                //  [head..FCU.number]

                if self
                    .try_insert_new_payload(block)
                    .map(|status| status.is_valid())
                    .unwrap_or_default()
                {
                    // payload is valid
                    self.sync_state_updater.update_sync_state(SyncState::Idle);
                } else if let Some(target) = self.forkchoice_state_tracker.sync_target() {
                    // if the payload is invalid, we run the pipeline to the head block.
                    self.sync.set_pipeline_sync_target(target);
                }
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
                                        return Some(Err(Error::Provider(
                                            ProviderError::HeaderNotFound(max_block.into()),
                                        )
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

                        let sync_target_state = match self
                            .forkchoice_state_tracker
                            .sync_target_state()
                        {
                            Some(current_state) => current_state,
                            None => {
                                // This is only possible if the node was run with `debug.tip`
                                // argument and without CL.
                                warn!(target: "consensus::engine", "No forkchoice state available");
                                return None
                            }
                        };

                        // TODO: figure out how to make this less complex:
                        // restore_tree_if_possible will run the pipeline if the current_state head
                        // hash is missing. This can arise if we buffer the forkchoice head, and if
                        // the head is an ancestor of an invalid block.
                        //
                        //  * The forkchoice head could be buffered if it were first sent as a
                        //    `newPayload` request.
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
                            match self.restore_tree_if_possible(sync_target_state) {
                                Ok(_) => self.sync_state_updater.update_sync_state(SyncState::Idle),
                                Err(error) => {
                                    error!(target: "consensus::engine", ?error, "Error restoring blockchain tree");
                                    return Some(Err(error.into()))
                                }
                            };
                        }
                    }
                    // Any pipeline error at this point is fatal.
                    Err(error) => return Some(Err(error.into())),
                };
            }
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
        + BlockProvider
        + CanonChainTracker
        + StageCheckpointProvider
        + Unpin
        + 'static,
{
    type Output = Result<(), BeaconConsensusEngineError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        // Process all incoming messages first.
        while let Poll::Ready(Some(msg)) = this.engine_message_rx.poll_next_unpin(cx) {
            match msg {
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
                BeaconEngineMessage::EventListener(tx) => {
                    this.listeners.push_listener(tx);
                }
            }
        }

        // poll sync controller
        while let Poll::Ready(sync_event) = this.sync.poll(cx) {
            if let Some(res) = this.on_sync_event(sync_event) {
                return Poll::Ready(res)
            }
        }

        Poll::Pending
    }
}

/// Keeps track of invalid headers.
struct InvalidHeaderCache {
    /// This maps a header hash to a reference to its invalid ancestor.
    headers: LruMap<H256, Arc<Header>>,
}

impl InvalidHeaderCache {
    fn new(max_length: u32) -> Self {
        Self { headers: LruMap::new(ByLength::new(max_length)) }
    }

    /// Returns the invalid ancestor's header if it exists in the cache.
    fn get(&mut self, hash: &H256) -> Option<&mut Arc<Header>> {
        self.headers.get(hash)
    }

    /// Inserts an invalid block into the cache, with a given invalid ancestor.
    fn insert_with_invalid_ancestor(&mut self, header_hash: H256, invalid_ancestor: Arc<Header>) {
        self.headers.insert(header_hash, invalid_ancestor);
    }

    /// Inserts an invalid ancestor into the map.
    fn insert(&mut self, invalid_ancestor: SealedHeader) {
        let hash = invalid_ancestor.hash;
        let header = invalid_ancestor.unseal();
        self.headers.insert(hash, Arc::new(header));
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
    use reth_db::mdbx::{test_utils::create_test_rw_db, Env, WriteMap};
    use reth_interfaces::{
        sync::NoopSyncStateUpdater,
        test_utils::{NoopFullBlockClient, TestConsensus},
    };
    use reth_payload_builder::test_utils::spawn_test_payload_service;
    use reth_primitives::{
        stage::StageCheckpoint, ChainSpec, ChainSpecBuilder, SealedBlockWithSenders, H256, MAINNET,
    };
    use reth_provider::{
        providers::BlockchainProvider, test_utils::TestExecutorFactory, ShareableDatabase,
        Transaction,
    };
    use reth_stages::{test_utils::TestStages, ExecOutput, PipelineError, StageError};
    use reth_tasks::TokioTaskExecutor;
    use std::{collections::VecDeque, sync::Arc, time::Duration};
    use tokio::sync::{
        oneshot::{self, error::TryRecvError},
        watch,
    };

    type TestBeaconConsensusEngine = BeaconConsensusEngine<
        Arc<Env<WriteMap>>,
        BlockchainProvider<
            Arc<Env<WriteMap>>,
            ShareableBlockchainTree<Arc<Env<WriteMap>>, TestConsensus, TestExecutorFactory>,
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

    fn setup_consensus_engine(
        chain_spec: Arc<ChainSpec>,
        pipeline_exec_outputs: VecDeque<Result<ExecOutput, StageError>>,
        executor_results: Vec<PostState>,
    ) -> (TestBeaconConsensusEngine, TestEnv<Arc<Env<WriteMap>>>) {
        reth_tracing::init_test_tracing();
        let db = create_test_rw_db();
        let consensus = TestConsensus::default();
        let payload_builder = spawn_test_payload_service();

        let executor_factory = TestExecutorFactory::new(chain_spec.clone());
        executor_factory.extend(executor_results);

        // Setup pipeline
        let (tip_tx, tip_rx) = watch::channel(H256::default());
        let pipeline = Pipeline::builder()
            .add_stages(TestStages::new(pipeline_exec_outputs, Default::default()))
            .with_tip_sender(tip_tx)
            .build(db.clone());

        // Setup blockchain tree
        let externals =
            TreeExternals::new(db.clone(), consensus, executor_factory, chain_spec.clone());
        let config = BlockchainTreeConfig::new(1, 2, 3, 2);
        let (canon_state_notification_sender, _) = tokio::sync::broadcast::channel(3);
        let tree = ShareableBlockchainTree::new(
            BlockchainTree::new(externals, canon_state_notification_sender, config)
                .expect("failed to create tree"),
        );
        let shareable_db = ShareableDatabase::new(db.clone(), chain_spec.clone());
        let latest = chain_spec.genesis_header().seal_slow();
        let blockchain_provider = BlockchainProvider::with_latest(shareable_db, tree, latest);
        let (engine, handle) = BeaconConsensusEngine::new(
            NoopFullBlockClient::default(),
            pipeline,
            blockchain_provider,
            Box::<TokioTaskExecutor>::default(),
            Box::<NoopSyncStateUpdater>::default(),
            None,
            false,
            payload_builder,
            None,
        )
        .expect("failed to create consensus engine");

        (engine, TestEnv::new(db, tip_rx, handle))
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
        let (consensus_engine, env) = setup_consensus_engine(
            chain_spec,
            VecDeque::from([Err(StageError::ChannelClosed)]),
            Vec::default(),
        );
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
        let (consensus_engine, env) = setup_consensus_engine(
            chain_spec,
            VecDeque::from([Err(StageError::ChannelClosed)]),
            Vec::default(),
        );
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
        let (consensus_engine, env) = setup_consensus_engine(
            chain_spec,
            VecDeque::from([
                Ok(ExecOutput { checkpoint: StageCheckpoint::new(1), done: true }),
                Err(StageError::ChannelClosed),
            ]),
            Vec::default(),
        );
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
        let (mut consensus_engine, env) = setup_consensus_engine(
            chain_spec,
            VecDeque::from([Ok(ExecOutput {
                checkpoint: StageCheckpoint::new(max_block),
                done: true,
            })]),
            Vec::default(),
        );
        consensus_engine.sync.set_max_block(max_block);
        let rx = spawn_consensus_engine(consensus_engine);

        let _ = env
            .send_forkchoice_updated(ForkchoiceState {
                head_block_hash: H256::random(),
                ..Default::default()
            })
            .await;
        assert_matches!(rx.await, Ok(Ok(())));
    }

    fn insert_blocks<'a, DB: Database>(db: &DB, mut blocks: impl Iterator<Item = &'a SealedBlock>) {
        let mut transaction = Transaction::new(db).unwrap();
        blocks
            .try_for_each(|b| {
                transaction
                    .insert_block(SealedBlockWithSenders::new(b.clone(), Vec::default()).unwrap())
            })
            .expect("failed to insert");
        transaction.commit().unwrap();
    }

    mod fork_choice_updated {
        use super::*;
        use reth_db::{tables, transaction::DbTxMut};
        use reth_interfaces::test_utils::generators::random_block;
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
            let (consensus_engine, env) = setup_consensus_engine(
                chain_spec,
                VecDeque::from([Ok(ExecOutput {
                    done: true,
                    checkpoint: StageCheckpoint::new(0),
                })]),
                Vec::default(),
            );

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
            let chain_spec = Arc::new(
                ChainSpecBuilder::default()
                    .chain(MAINNET.chain)
                    .genesis(MAINNET.genesis.clone())
                    .paris_activated()
                    .build(),
            );
            let (consensus_engine, env) = setup_consensus_engine(
                chain_spec,
                VecDeque::from([Ok(ExecOutput {
                    done: true,
                    checkpoint: StageCheckpoint::new(0),
                })]),
                Vec::default(),
            );

            let genesis = random_block(0, None, None, Some(0));
            let block1 = random_block(1, Some(genesis.hash), None, Some(0));
            insert_blocks(env.db.as_ref(), [&genesis, &block1].into_iter());
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
            let chain_spec = Arc::new(
                ChainSpecBuilder::default()
                    .chain(MAINNET.chain)
                    .genesis(MAINNET.genesis.clone())
                    .paris_activated()
                    .build(),
            );
            let (consensus_engine, env) = setup_consensus_engine(
                chain_spec,
                VecDeque::from([
                    Ok(ExecOutput { done: true, checkpoint: StageCheckpoint::new(0) }),
                    Ok(ExecOutput { done: true, checkpoint: StageCheckpoint::new(0) }),
                ]),
                Vec::default(),
            );

            let genesis = random_block(0, None, None, Some(0));
            let block1 = random_block(1, Some(genesis.hash), None, Some(0));
            insert_blocks(env.db.as_ref(), [&genesis, &block1].into_iter());

            let mut engine_rx = spawn_consensus_engine(consensus_engine);

            let next_head = random_block(2, Some(block1.hash), None, Some(0));
            let next_forkchoice_state = ForkchoiceState {
                head_block_hash: next_head.hash,
                finalized_block_hash: block1.hash,
                ..Default::default()
            };

            // if we `await` in the assert, the forkchoice will poll after we've inserted the block,
            // and it will return VALID instead of SYNCING
            let invalid_rx = env.send_forkchoice_updated(next_forkchoice_state).await;

            // Insert next head immediately after sending forkchoice update
            insert_blocks(env.db.as_ref(), [&next_head].into_iter());

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
            let chain_spec = Arc::new(
                ChainSpecBuilder::default()
                    .chain(MAINNET.chain)
                    .genesis(MAINNET.genesis.clone())
                    .paris_activated()
                    .build(),
            );
            let (consensus_engine, env) = setup_consensus_engine(
                chain_spec,
                VecDeque::from([Ok(ExecOutput {
                    done: true,
                    checkpoint: StageCheckpoint::new(0),
                })]),
                Vec::default(),
            );

            let genesis = random_block(0, None, None, Some(0));
            let block1 = random_block(1, Some(genesis.hash), None, Some(0));
            insert_blocks(env.db.as_ref(), [&genesis, &block1].into_iter());

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
            let chain_spec = Arc::new(
                ChainSpecBuilder::default()
                    .chain(MAINNET.chain)
                    .genesis(MAINNET.genesis.clone())
                    .london_activated()
                    .paris_at_ttd(U256::from(3))
                    .build(),
            );
            let (consensus_engine, env) = setup_consensus_engine(
                chain_spec,
                VecDeque::from([
                    Ok(ExecOutput { done: true, checkpoint: StageCheckpoint::new(0) }),
                    Ok(ExecOutput { done: true, checkpoint: StageCheckpoint::new(0) }),
                ]),
                Vec::default(),
            );

            let genesis = random_block(0, None, None, Some(0));
            let mut block1 = random_block(1, Some(genesis.hash), None, Some(0));
            block1.header.difficulty = U256::from(1);

            // a second pre-merge block
            let mut block2 = random_block(1, Some(genesis.hash), None, Some(0));
            block2.header.difficulty = U256::from(1);

            // a transition block
            let mut block3 = random_block(1, Some(genesis.hash), None, Some(0));
            block3.header.difficulty = U256::from(1);

            insert_blocks(env.db.as_ref(), [&genesis, &block1, &block2, &block3].into_iter());

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
            let chain_spec = Arc::new(
                ChainSpecBuilder::default()
                    .chain(MAINNET.chain)
                    .genesis(MAINNET.genesis.clone())
                    .london_activated()
                    .build(),
            );
            let (consensus_engine, env) = setup_consensus_engine(
                chain_spec,
                VecDeque::from([
                    Ok(ExecOutput { done: true, checkpoint: StageCheckpoint::new(0) }),
                    Ok(ExecOutput { done: true, checkpoint: StageCheckpoint::new(0) }),
                ]),
                Vec::default(),
            );

            let genesis = random_block(0, None, None, Some(0));
            let block1 = random_block(1, Some(genesis.hash), None, Some(0));

            insert_blocks(env.db.as_ref(), [&genesis, &block1].into_iter());

            let _engine = spawn_consensus_engine(consensus_engine);

            let res = env
                .send_forkchoice_updated(ForkchoiceState {
                    head_block_hash: block1.hash,
                    finalized_block_hash: block1.hash,
                    ..Default::default()
                })
                .await;
            let expected_result = ForkchoiceUpdated::from_status(PayloadStatusEnum::Invalid {
                validation_error: BlockExecutionError::BlockPreMerge { hash: block1.hash }
                    .to_string(),
            })
            .with_latest_valid_hash(H256::zero());
            assert_matches!(res, Ok(result) => assert_eq!(result, expected_result));
        }
    }

    mod new_payload {
        use super::*;
        use reth_interfaces::{
            executor::BlockExecutionError, test_utils::generators::random_block,
        };
        use reth_primitives::{Hardfork, U256};
        use reth_provider::test_utils::blocks::BlockChainTestData;

        #[tokio::test]
        async fn new_payload_before_forkchoice() {
            let chain_spec = Arc::new(
                ChainSpecBuilder::default()
                    .chain(MAINNET.chain)
                    .genesis(MAINNET.genesis.clone())
                    .paris_activated()
                    .build(),
            );
            let (consensus_engine, env) = setup_consensus_engine(
                chain_spec,
                VecDeque::from([Ok(ExecOutput {
                    done: true,
                    checkpoint: StageCheckpoint::new(0),
                })]),
                Vec::default(),
            );

            let mut engine_rx = spawn_consensus_engine(consensus_engine);

            // Send new payload
            let res = env.send_new_payload(random_block(0, None, None, Some(0)).into()).await;
            // Invalid, because this is a genesis block
            assert_matches!(res, Ok(result) => assert_matches!(result.status, PayloadStatusEnum::Invalid { .. }));

            // Send new payload
            let res = env.send_new_payload(random_block(1, None, None, Some(0)).into()).await;
            let expected_result = PayloadStatus::from_status(PayloadStatusEnum::Syncing);
            assert_matches!(res, Ok(result) => assert_eq!(result, expected_result));

            assert_matches!(engine_rx.try_recv(), Err(TryRecvError::Empty));
        }

        #[tokio::test]
        async fn payload_known() {
            let chain_spec = Arc::new(
                ChainSpecBuilder::default()
                    .chain(MAINNET.chain)
                    .genesis(MAINNET.genesis.clone())
                    .paris_activated()
                    .build(),
            );
            let (consensus_engine, env) = setup_consensus_engine(
                chain_spec,
                VecDeque::from([Ok(ExecOutput {
                    done: true,
                    checkpoint: StageCheckpoint::new(0),
                })]),
                Vec::default(),
            );

            let genesis = random_block(0, None, None, Some(0));
            let block1 = random_block(1, Some(genesis.hash), None, Some(0));
            let block2 = random_block(2, Some(block1.hash), None, Some(0));
            insert_blocks(env.db.as_ref(), [&genesis, &block1, &block2].into_iter());

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
            let chain_spec = Arc::new(
                ChainSpecBuilder::default()
                    .chain(MAINNET.chain)
                    .genesis(MAINNET.genesis.clone())
                    .paris_activated()
                    .build(),
            );
            let (consensus_engine, env) = setup_consensus_engine(
                chain_spec,
                VecDeque::from([Ok(ExecOutput {
                    done: true,
                    checkpoint: StageCheckpoint::new(0),
                })]),
                Vec::default(),
            );

            let genesis = random_block(0, None, None, Some(0));

            insert_blocks(env.db.as_ref(), [&genesis].into_iter());

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
            let block = random_block(2, Some(H256::random()), None, Some(0));
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
            let (consensus_engine, env) = setup_consensus_engine(
                chain_spec,
                VecDeque::from([Ok(ExecOutput {
                    done: true,
                    checkpoint: StageCheckpoint::new(0),
                })]),
                Vec::from([exec_result2]),
            );

            insert_blocks(env.db.as_ref(), [&data.genesis, &block1].into_iter());

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
                validation_error: BlockExecutionError::BlockPreMerge { hash: block1.hash }
                    .to_string(),
            })
            .with_latest_valid_hash(H256::zero());
            assert_matches!(res, Ok(ForkchoiceUpdated { payload_status, .. }) => assert_eq!(payload_status, expected_result));

            // Send new payload
            let result =
                env.send_new_payload_retry_on_syncing(block2.clone().into()).await.unwrap();

            let expected_result = PayloadStatus::from_status(PayloadStatusEnum::Invalid {
                validation_error: BlockExecutionError::BlockPreMerge { hash: block2.hash }
                    .to_string(),
            })
            .with_latest_valid_hash(H256::zero());
            assert_eq!(result, expected_result);

            assert_matches!(engine_rx.try_recv(), Err(TryRecvError::Empty));
        }
    }
}

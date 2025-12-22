use crate::{
    error::BeaconForkChoiceUpdateError, BeaconOnNewPayloadError, ExecutionPayload, ForkchoiceStatus,
};
use alloy_rpc_types_engine::{
    ForkChoiceUpdateResult, ForkchoiceState, ForkchoiceUpdateError, ForkchoiceUpdated, PayloadId,
    PayloadStatus, PayloadStatusEnum,
};
use core::{
    fmt::{self, Display},
    future::Future,
    pin::Pin,
    task::{ready, Context, Poll},
};
use futures::{future::Either, FutureExt, TryFutureExt};
use reth_chain_state::ExecutedBlock;
use reth_errors::RethResult;
use reth_payload_builder_primitives::PayloadBuilderError;
use reth_payload_primitives::{BuiltPayload, EngineApiMessageVersion, PayloadTypes};
use tokio::sync::{mpsc::UnboundedSender, oneshot};

/// Type alias for backwards compat
#[deprecated(note = "Use ConsensusEngineHandle instead")]
pub type BeaconConsensusEngineHandle<Payload> = ConsensusEngineHandle<Payload>;

/// Represents the outcome of forkchoice update.
///
/// This is a future that resolves to [`ForkChoiceUpdateResult`]
#[must_use = "futures do nothing unless you `.await` or poll them"]
#[derive(Debug)]
pub struct OnForkChoiceUpdated {
    /// Represents the status of the forkchoice update.
    ///
    /// Note: This is separate from the response `fut`, because we still can return an error
    /// depending on the payload attributes, even if the forkchoice update itself is valid.
    forkchoice_status: ForkchoiceStatus,
    /// Returns the result of the forkchoice update.
    fut: Either<futures::future::Ready<ForkChoiceUpdateResult>, PendingPayloadId>,
}

// === impl OnForkChoiceUpdated ===

impl OnForkChoiceUpdated {
    /// Returns the determined status of the received `ForkchoiceState`.
    pub const fn forkchoice_status(&self) -> ForkchoiceStatus {
        self.forkchoice_status
    }

    /// Creates a new instance of `OnForkChoiceUpdated` for the `SYNCING` state
    pub fn syncing() -> Self {
        let status = PayloadStatus::from_status(PayloadStatusEnum::Syncing);
        Self {
            forkchoice_status: ForkchoiceStatus::from_payload_status(&status.status),
            fut: Either::Left(futures::future::ready(Ok(ForkchoiceUpdated::new(status)))),
        }
    }

    /// Creates a new instance of `OnForkChoiceUpdated` if the forkchoice update succeeded and no
    /// payload attributes were provided.
    pub fn valid(status: PayloadStatus) -> Self {
        Self {
            forkchoice_status: ForkchoiceStatus::from_payload_status(&status.status),
            fut: Either::Left(futures::future::ready(Ok(ForkchoiceUpdated::new(status)))),
        }
    }

    /// Creates a new instance of `OnForkChoiceUpdated` with the given payload status, if the
    /// forkchoice update failed due to an invalid payload.
    pub fn with_invalid(status: PayloadStatus) -> Self {
        Self {
            forkchoice_status: ForkchoiceStatus::from_payload_status(&status.status),
            fut: Either::Left(futures::future::ready(Ok(ForkchoiceUpdated::new(status)))),
        }
    }

    /// Creates a new instance of `OnForkChoiceUpdated` if the forkchoice update failed because the
    /// given state is considered invalid
    pub fn invalid_state() -> Self {
        Self {
            forkchoice_status: ForkchoiceStatus::Invalid,
            fut: Either::Left(futures::future::ready(Err(ForkchoiceUpdateError::InvalidState))),
        }
    }

    /// Creates a new instance of `OnForkChoiceUpdated` if the forkchoice update was successful but
    /// payload attributes were invalid.
    pub fn invalid_payload_attributes() -> Self {
        Self {
            // This is valid because this is only reachable if the state and payload is valid
            forkchoice_status: ForkchoiceStatus::Valid,
            fut: Either::Left(futures::future::ready(Err(
                ForkchoiceUpdateError::UpdatedInvalidPayloadAttributes,
            ))),
        }
    }

    /// If the forkchoice update was successful and no payload attributes were provided, this method
    pub const fn updated_with_pending_payload_id(
        payload_status: PayloadStatus,
        pending_payload_id: oneshot::Receiver<Result<PayloadId, PayloadBuilderError>>,
    ) -> Self {
        Self {
            forkchoice_status: ForkchoiceStatus::from_payload_status(&payload_status.status),
            fut: Either::Right(PendingPayloadId {
                payload_status: Some(payload_status),
                pending_payload_id,
            }),
        }
    }
}

impl Future for OnForkChoiceUpdated {
    type Output = ForkChoiceUpdateResult;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.get_mut().fut.poll_unpin(cx)
    }
}

/// A future that returns the payload id of a yet to be initiated payload job after a successful
/// forkchoice update
#[derive(Debug)]
struct PendingPayloadId {
    payload_status: Option<PayloadStatus>,
    pending_payload_id: oneshot::Receiver<Result<PayloadId, PayloadBuilderError>>,
}

impl Future for PendingPayloadId {
    type Output = ForkChoiceUpdateResult;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let res = ready!(this.pending_payload_id.poll_unpin(cx));
        match res {
            Ok(Ok(payload_id)) => Poll::Ready(Ok(ForkchoiceUpdated {
                payload_status: this.payload_status.take().expect("Polled after completion"),
                payload_id: Some(payload_id),
            })),
            Err(_) | Ok(Err(_)) => {
                // failed to initiate a payload build job
                Poll::Ready(Err(ForkchoiceUpdateError::UpdatedInvalidPayloadAttributes))
            }
        }
    }
}

/// A message for the beacon engine from other components of the node (engine RPC API invoked by the
/// consensus layer).
#[derive(Debug)]
pub enum BeaconEngineMessage<Payload: PayloadTypes> {
    /// Message with new payload.
    NewPayload {
        /// The execution payload received by Engine API.
        payload: Payload::ExecutionData,
        /// The sender for returning payload status result.
        tx: oneshot::Sender<Result<PayloadStatus, BeaconOnNewPayloadError>>,
    },
    /// Message with updated forkchoice state.
    ForkchoiceUpdated {
        /// The updated forkchoice state.
        state: ForkchoiceState,
        /// The payload attributes for block building.
        payload_attrs: Option<Payload::PayloadAttributes>,
        /// The Engine API Version.
        version: EngineApiMessageVersion,
        /// The sender for returning forkchoice updated result.
        tx: oneshot::Sender<RethResult<OnForkChoiceUpdated>>,
    },
    /// Message to insert a locally built executed block into the engine state tree.
    InsertExecutedBlock {
        /// The executed block to insert.
        block: ExecutedBlock<<Payload::BuiltPayload as BuiltPayload>::Primitives>,
    },
}

impl<Payload: PayloadTypes> Display for BeaconEngineMessage<Payload> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NewPayload { payload, .. } => {
                write!(
                    f,
                    "NewPayload(parent: {}, number: {}, hash: {})",
                    payload.parent_hash(),
                    payload.block_number(),
                    payload.block_hash()
                )
            }
            Self::ForkchoiceUpdated { state, payload_attrs, .. } => {
                // we don't want to print the entire payload attributes, because for OP this
                // includes all txs
                write!(
                    f,
                    "ForkchoiceUpdated {{ state: {state:?}, has_payload_attributes: {} }}",
                    payload_attrs.is_some()
                )
            }
            Self::InsertExecutedBlock { block } => {
                let num_hash = block.recovered_block().num_hash();
                write!(
                    f,
                    "InsertExecutedBlock(number: {}, hash: {})",
                    num_hash.number, num_hash.hash,
                )
            }
        }
    }
}

/// A cloneable sender type that can be used to send engine API messages.
///
/// This type mirrors consensus related functions of the engine API.
#[derive(Debug, Clone)]
pub struct ConsensusEngineHandle<Payload>
where
    Payload: PayloadTypes,
{
    to_engine: UnboundedSender<BeaconEngineMessage<Payload>>,
}

impl<Payload> ConsensusEngineHandle<Payload>
where
    Payload: PayloadTypes,
{
    /// Creates a new beacon consensus engine handle.
    pub const fn new(to_engine: UnboundedSender<BeaconEngineMessage<Payload>>) -> Self {
        Self { to_engine }
    }

    /// Sends a new payload message to the beacon consensus engine and waits for a response.
    ///
    /// See also <https://github.com/ethereum/execution-apis/blob/3d627c95a4d3510a8187dd02e0250ecb4331d27e/src/engine/shanghai.md#engine_newpayloadv2>
    pub async fn new_payload(
        &self,
        payload: Payload::ExecutionData,
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
        payload_attrs: Option<Payload::PayloadAttributes>,
        version: EngineApiMessageVersion,
    ) -> Result<ForkchoiceUpdated, BeaconForkChoiceUpdateError> {
        Ok(self
            .send_fork_choice_updated(state, payload_attrs, version)
            .map_err(|_| BeaconForkChoiceUpdateError::EngineUnavailable)
            .await?
            .map_err(BeaconForkChoiceUpdateError::internal)?
            .await?)
    }

    /// Sends a forkchoice update message to the beacon consensus engine and returns the receiver to
    /// wait for a response.
    fn send_fork_choice_updated(
        &self,
        state: ForkchoiceState,
        payload_attrs: Option<Payload::PayloadAttributes>,
        version: EngineApiMessageVersion,
    ) -> oneshot::Receiver<RethResult<OnForkChoiceUpdated>> {
        let (tx, rx) = oneshot::channel();
        let _ = self.to_engine.send(BeaconEngineMessage::ForkchoiceUpdated {
            state,
            payload_attrs,
            tx,
            version,
        });
        rx
    }

    /// Sends a message to insert the executed block into the engine state tree.
    pub fn send_insert_executed_block(
        &self,
        block: ExecutedBlock<<Payload::BuiltPayload as BuiltPayload>::Primitives>,
    ) {
        let _ = self.to_engine.send(BeaconEngineMessage::InsertExecutedBlock { block });
    }
}

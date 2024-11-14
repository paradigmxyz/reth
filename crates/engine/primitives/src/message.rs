use crate::{BeaconOnNewPayloadError, EngineApiMessageVersion, EngineTypes, ForkchoiceStatus};
use alloy_rpc_types_engine::{
    ExecutionPayload, ExecutionPayloadSidecar, ForkChoiceUpdateResult, ForkchoiceState,
    ForkchoiceUpdateError, ForkchoiceUpdated, PayloadId, PayloadStatus, PayloadStatusEnum,
};
use futures::{future::Either, FutureExt};
use reth_errors::RethResult;
use reth_payload_primitives::PayloadBuilderError;
use std::{
    fmt::Display,
    future::Future,
    pin::Pin,
    task::{ready, Context, Poll},
};
use tokio::sync::oneshot;

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
pub enum BeaconEngineMessage<Engine: EngineTypes> {
    /// Message with new payload.
    NewPayload {
        /// The execution payload received by Engine API.
        payload: ExecutionPayload,
        /// The execution payload sidecar with additional version-specific fields received by
        /// engine API.
        sidecar: ExecutionPayloadSidecar,
        /// The sender for returning payload status result.
        tx: oneshot::Sender<Result<PayloadStatus, BeaconOnNewPayloadError>>,
    },
    /// Message with updated forkchoice state.
    ForkchoiceUpdated {
        /// The updated forkchoice state.
        state: ForkchoiceState,
        /// The payload attributes for block building.
        payload_attrs: Option<Engine::PayloadAttributes>,
        /// The Engine API Version.
        version: EngineApiMessageVersion,
        /// The sender for returning forkchoice updated result.
        tx: oneshot::Sender<RethResult<OnForkChoiceUpdated>>,
    },
    /// Message with exchanged transition configuration.
    TransitionConfigurationExchanged,
}

impl<Engine: EngineTypes> Display for BeaconEngineMessage<Engine> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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
            Self::TransitionConfigurationExchanged => {
                write!(f, "TransitionConfigurationExchanged")
            }
        }
    }
}

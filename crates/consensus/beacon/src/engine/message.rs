use crate::BeaconEngineResult;
use reth_interfaces::consensus::ForkchoiceState;
use reth_rpc_types::engine::{
    ExecutionPayload, ExecutionPayloadEnvelope, ForkchoiceUpdated, PayloadAttributes, PayloadId,
    PayloadStatus,
};
use tokio::sync::oneshot;

/// Beacon engine sender.
pub type BeaconEngineSender<Ok> = oneshot::Sender<BeaconEngineResult<Ok>>;

/// A message for the beacon engine from other components of the node.
#[derive(Debug)]
pub enum BeaconEngineMessage {
    /// Message with new payload.
    NewPayload {
        /// The execution payload received by Engine API.
        payload: ExecutionPayload,
        /// The sender for returning payload status result.
        tx: BeaconEngineSender<PayloadStatus>,
    },
    /// Message with updated forkchoice state.
    ForkchoiceUpdated {
        /// The updated forkchoice state.
        state: ForkchoiceState,
        /// The payload attributes for block building.
        payload_attrs: Option<PayloadAttributes>,
        /// The sender for returning forkchoice updated result.
        tx: BeaconEngineSender<ForkchoiceUpdated>,
    },
    /// Message with get payload parameters.
    GetPayload {
        /// The payload id.
        payload_id: PayloadId,
        /// The sender for returning payload result.
        tx: BeaconEngineSender<ExecutionPayloadEnvelope>,
    },
}

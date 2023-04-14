use crate::BeaconEngineResult;
use reth_interfaces::consensus::ForkchoiceState;
use reth_rpc_types::engine::{
    ExecutionPayload, ForkchoiceUpdated, PayloadAttributes, PayloadStatus,
};
use tokio::sync::oneshot;

/// A message for the beacon engine from other components of the node (engine RPC API invoked by the
/// consensus layer).
#[derive(Debug)]
pub enum BeaconEngineMessage {
    /// Message with new payload.
    NewPayload {
        /// The execution payload received by Engine API.
        payload: ExecutionPayload,
        /// The sender for returning payload status result.
        tx: oneshot::Sender<BeaconEngineResult<PayloadStatus>>,
    },
    /// Message with updated forkchoice state.
    ForkchoiceUpdated {
        /// The updated forkchoice state.
        state: ForkchoiceState,
        /// The payload attributes for block building.
        payload_attrs: Option<PayloadAttributes>,
        /// The sender for returning forkchoice updated result.
        tx: oneshot::Sender<BeaconEngineResult<ForkchoiceUpdated>>,
    },
}

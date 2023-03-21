use crate::BeaconEngineResult;
use reth_interfaces::consensus::ForkchoiceState;
use reth_rpc_types::engine::{
    ExecutionPayload, ForkchoiceUpdated, PayloadAttributes, PayloadStatus,
};
use tokio::sync::oneshot;

/// Beacon engine sender.
pub type BeaconEngineSender<Ok> = oneshot::Sender<BeaconEngineResult<Ok>>;

/// The message received by beacon consensus engine.
#[derive(Debug)]
pub enum BeaconEngineMessage {
    /// Message with new payload.
    NewPayload(ExecutionPayload, BeaconEngineSender<PayloadStatus>),
    /// Message with updated forkchoice state.
    ForkchoiceUpdated(
        ForkchoiceState,
        Option<PayloadAttributes>,
        BeaconEngineSender<ForkchoiceUpdated>,
    ),
}

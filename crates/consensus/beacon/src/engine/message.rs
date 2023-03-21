use crate::BeaconEngineResult;
use reth_interfaces::consensus::ForkchoiceState;
use reth_primitives::SealedBlock;
use reth_rpc_types::engine::{ForkchoiceUpdated, PayloadAttributes, PayloadStatus};
use tokio::sync::oneshot;

/// Beacon engine sender.
pub type BeaconEngineSender<Ok> = oneshot::Sender<BeaconEngineResult<Ok>>;

/// The message received by beacon consensus engine.
#[derive(Debug)]
pub enum BeaconEngineMessage {
    /// Message with new payload.
    NewPayload(SealedBlock, BeaconEngineSender<PayloadStatus>),
    /// Message with updated forkchoice state.
    ForkchoiceUpdated(
        ForkchoiceState,
        Option<PayloadAttributes>,
        BeaconEngineSender<ForkchoiceUpdated>,
    ),
}

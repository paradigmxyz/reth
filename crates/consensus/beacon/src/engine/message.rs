use reth_interfaces::consensus::ForkchoiceState;
use reth_primitives::SealedBlock;
use reth_rpc_types::engine::PayloadStatusEnum;
use tokio::sync::oneshot;

/// The message received by beacon consensus engine.
#[derive(Debug)]
pub enum BeaconEngineMessage {
    /// Message with updated forkchoice state.
    ForkchoiceUpdated(ForkchoiceState, oneshot::Sender<PayloadStatusEnum>),
    /// Message with new payload.
    NewPayload(SealedBlock, oneshot::Sender<PayloadStatusEnum>),
}

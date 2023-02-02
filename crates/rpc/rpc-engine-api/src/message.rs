use crate::EngineApiSender;
use reth_interfaces::consensus::ForkchoiceState;
use reth_primitives::H64;
use reth_rpc_types::engine::{
    ExecutionPayload, ForkchoiceUpdated, PayloadAttributes, PayloadStatus, TransitionConfiguration,
};

/// Message type for communicating with [`EngineApi`][crate::EngineApi].
#[derive(Debug)]
pub enum EngineApiMessage {
    /// New payload message
    NewPayload(ExecutionPayload, EngineApiSender<PayloadStatus>),
    /// Get payload message
    GetPayload(H64, EngineApiSender<ExecutionPayload>),
    /// Forkchoice updated message
    ForkchoiceUpdated(
        ForkchoiceState,
        Option<PayloadAttributes>,
        EngineApiSender<ForkchoiceUpdated>,
    ),
    /// Exchange transition configuration message
    ExchangeTransitionConfiguration(
        TransitionConfiguration,
        EngineApiSender<TransitionConfiguration>,
    ),
}

use crate::EngineApiSender;
use reth_interfaces::consensus::ForkchoiceState;
use reth_primitives::{BlockHash, BlockNumber, H64};
use reth_rpc_types::engine::{
    ExecutionPayload, ExecutionPayloadBody, ForkchoiceUpdated, PayloadAttributes, PayloadStatus,
    TransitionConfiguration,
};

/// Message type for communicating with [`EngineApi`][crate::EngineApi].
#[derive(Debug)]
pub enum EngineApiMessage {
    /// New payload message
    NewPayload(ExecutionPayload, EngineApiSender<PayloadStatus>),
    /// Get payload message
    GetPayload(H64, EngineApiSender<ExecutionPayload>),
    /// Get payload bodies by range message
    GetPayloadBodiesByRange(BlockNumber, u64, EngineApiSender<Vec<ExecutionPayloadBody>>),
    /// Get payload bodies by hash message
    GetPayloadBodiesByHash(Vec<BlockHash>, EngineApiSender<Vec<ExecutionPayloadBody>>),
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

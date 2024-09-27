use op_alloy_rpc_types_engine::OptimismExecutionPayloadEnvelopeV3;
use reth::rpc::types::engine::{ExecutionPayloadEnvelopeV3, ExecutionPayloadV3};

/// The execution payload envelope type.
pub trait PayloadEnvelopeExt: Send + Sync + std::fmt::Debug {
    /// Returns the execution payload V3 from the payload
    fn execution_payload(&self) -> ExecutionPayloadV3;
}

impl PayloadEnvelopeExt for OptimismExecutionPayloadEnvelopeV3 {
    fn execution_payload(&self) -> ExecutionPayloadV3 {
        self.execution_payload.clone()
    }
}

impl PayloadEnvelopeExt for ExecutionPayloadEnvelopeV3 {
    fn execution_payload(&self) -> ExecutionPayloadV3 {
        self.execution_payload.clone()
    }
}

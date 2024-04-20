use reth::rpc::types::{
    engine::{ExecutionPayloadEnvelopeV3, OptimismExecutionPayloadEnvelopeV3},
    ExecutionPayloadV3,
};

/// The execution payload envelope type.
pub trait PayloadEnvelopeV3: Send + Sync + std::fmt::Debug {
    /// Returns execution payload from envelope
    fn execution_payload(&self) -> ExecutionPayloadV3;
}

impl PayloadEnvelopeV3 for OptimismExecutionPayloadEnvelopeV3 {
    fn execution_payload(&self) -> ExecutionPayloadV3 {
        self.execution_payload.clone()
    }
}

impl PayloadEnvelopeV3 for ExecutionPayloadEnvelopeV3 {
    fn execution_payload(&self) -> ExecutionPayloadV3 {
        self.execution_payload.clone()
    }
}

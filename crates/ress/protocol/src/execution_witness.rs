use alloy_primitives::Bytes;
use alloy_rlp::{RlpDecodable, RlpEncodable};
use alloy_rpc_types_debug::ExecutionWitness;

/// Temporary wrapper around `ExecutionWitness` so that it is `RLPDecodable`
#[cfg_attr(any(test, feature = "arbitrary"), derive(arbitrary::Arbitrary))]
#[derive(Debug, RlpDecodable, RlpEncodable, Clone, Default, PartialEq, Eq)]
pub struct RLPExecutionWitness {
    /// Serialized hashed post state
    pub state: Vec<Bytes>,
    /// Serialized bytecode
    pub codes: Vec<Bytes>,
    /// Serialized headers
    pub headers: Vec<Bytes>,
}

impl From<ExecutionWitness> for RLPExecutionWitness {
    fn from(value: ExecutionWitness) -> Self {
        Self { state: value.state, codes: value.codes, headers: value.headers }
    }
}

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
    /// TODO: This is not needed for stateless if we are re-executing blocks since the client will
    /// TODO: store the previous blocks
    pub headers: Vec<Bytes>,
}

impl RLPExecutionWitness {
    /// Returns true if the witness is empty
    pub fn is_empty(&self) -> bool {
        self.state.is_empty() && self.codes.is_empty() && self.headers.is_empty()
    }

    /// Returns the total number of bytes needed to represent
    /// the whole witness
    pub fn size_with_headers(&self) -> usize {
        self.state.len() + self.codes.len() + self.headers.len()
    }

    /// Returns the total number of bytes needed to represent
    /// the witness minus the headers.
    ///
    /// Note: This is useful because if we are re-executing,
    /// we do not receive the headers and if we are verifying a proof,
    /// we do not receive the execution witness.
    ///
    /// TODO: We keep the headers for now to test the networking stack and
    /// TODO not make too many changes to the ExecutionWitness
    pub fn size_without_headers(&self) -> usize {
        self.state.len() + self.codes.len()
    }

    /// Returns the number of trie nodes in the witness
    pub fn node_count(&self) -> usize {
        self.state.len()
    }

    /// Returns the bytecodes in the witness
    pub fn code_count(&self) -> usize {
        self.codes.len()
    }

    /// Returns the average code size
    pub fn average_code_size(&self) -> usize {
        let total_code_size: usize = self.codes.iter().map(|code| code.len()).sum();
        total_code_size / self.code_count()
    }
}

impl From<ExecutionWitness> for RLPExecutionWitness {
    fn from(value: ExecutionWitness) -> Self {
        Self { state: value.state, codes: value.codes, headers: value.headers }
    }
}

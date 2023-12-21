//! Error types for the relay.

use alloy_primitives::B256;

/// Error thrown by the `validateBuilderSubmission` endpoints if the message differs from payload.
#[derive(Debug, thiserror::Error)]
pub enum ValidateBuilderSubmissionEqualityError {
    /// Thrown if parent hash mismatches
    #[error("incorrect ParentHash {actual}, expected {expected}")]
    IncorrectParentHash {
        /// The expected parent hash
        expected: B256,
        /// The actual parent hash
        actual: B256,
    },
    /// Thrown if block hash mismatches
    #[error("incorrect BlockHash {actual}, expected {expected}")]
    IncorrectBlockHash {
        /// The expected block hash
        expected: B256,
        /// The actual block hash
        actual: B256,
    },
    /// Thrown if block hash mismatches
    #[error("incorrect GasLimit {actual}, expected {expected}")]
    IncorrectGasLimit {
        /// The expected gas limit
        expected: u64,
        /// The actual gas limit
        actual: B256,
    },
    /// Thrown if block hash mismatches
    #[error("incorrect GasUsed {actual}, expected {expected}")]
    IncorrectGasUsed {
        /// The expected gas used
        expected: u64,
        /// The actual gas used
        actual: B256,
    },
}

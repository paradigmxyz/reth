//! Errors for the BAL execution path.

use alloy_evm::block::BlockExecutionError;
use alloy_primitives::B256;
use reth_provider::ProviderError;

/// Errors surfaced by `execute_block`.
#[derive(Debug, thiserror::Error)]
pub enum BalExecutionError {
    /// BAL-specific rejection.
    #[error("BAL rejection: {0:?}")]
    Reject(RejectReason),
    /// Worker or canonical EVM failure.
    ///
    /// This includes revm `BalError` for undeclared accesses. Revm does not yet report the
    /// offending address or storage slot here.
    #[error("evm execution failed: {0}")]
    Evm(#[from] BlockExecutionError),
    /// Provider setup failed before EVM execution could start.
    #[error("provider setup failed: {0}")]
    Provider(#[from] ProviderError),
    /// The received BAL could not be converted from alloy-format to revm-format.
    #[error("alloy→revm BAL conversion failed: {0}")]
    BalConversion(String),
}

impl From<RejectReason> for BalExecutionError {
    fn from(r: RejectReason) -> Self {
        Self::Reject(r)
    }
}

/// Reasons a block may be rejected on the BAL execution path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RejectReason {
    /// The rebuilt BAL's hash disagrees with `header.block_access_list_hash` at end-of-block.
    /// Catches over-declared addresses and any divergence the per-tx fragment compares missed.
    FinalHashMismatch {
        /// Hash of the rebuilt BAL.
        rebuilt: B256,
        /// Hash committed in the block header.
        expected: B256,
    },
}

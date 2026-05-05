//! Errors for the BAL execution path.

use alloy_evm::block::BlockExecutionError;
use alloy_primitives::B256;
use reth_provider::ProviderError;

/// Errors surfaced by `BalPayloadExecutor::execute_block`.
#[derive(Debug)]
pub enum BalExecutionError {
    /// BAL-specific rejection.
    Reject(RejectReason),
    /// Worker or canonical EVM failure.
    ///
    /// This includes revm `BalError` for undeclared accesses. Revm does not yet report the
    /// offending address or storage slot here.
    Evm(BlockExecutionError),
    /// Provider setup failed before EVM execution could start.
    Provider(ProviderError),
    /// The received BAL could not be converted from alloy-format to revm-format.
    BalConversion(String),
}

impl From<RejectReason> for BalExecutionError {
    fn from(r: RejectReason) -> Self {
        Self::Reject(r)
    }
}

impl From<BlockExecutionError> for BalExecutionError {
    fn from(e: BlockExecutionError) -> Self {
        Self::Evm(e)
    }
}

impl From<ProviderError> for BalExecutionError {
    fn from(e: ProviderError) -> Self {
        Self::Provider(e)
    }
}

impl core::fmt::Display for BalExecutionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Reject(r) => write!(f, "BAL rejection: {r:?}"),
            Self::Evm(e) => write!(f, "evm execution failed: {e}"),
            Self::Provider(e) => write!(f, "provider setup failed: {e}"),
            Self::BalConversion(s) => write!(f, "alloy→revm BAL conversion failed: {s}"),
        }
    }
}

impl core::error::Error for BalExecutionError {}

/// Reasons a block may be rejected on the BAL execution path.
///
/// These correspond to the validation stages described in `BAL.md`. Each stage maps to a specific
/// variant; producing an instance short-circuits block validation and propagates as an invalid
/// block error.
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

use reth_primitives::{Bloom, H256};
use thiserror::Error;

/// BlockExecutor Errors
#[allow(missing_docs)]
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum Error {
    #[error("EVM reported invalid transaction ({hash:?}): {message}")]
    EVM { hash: H256, message: String },
    #[error("Example of error.")]
    VerificationFailed,
    #[error("Fatal internal error")]
    ExecutionFatalError,
    #[error("Failed to recover sender for transaction")]
    SenderRecoveryError,
    #[error("Receipt cumulative gas used {got:?} is different from expected {expected:?}")]
    ReceiptCumulativeGasUsedDiff { got: u64, expected: u64 },
    #[error("Receipt log count {got:?} is different from expected {expected:?}.")]
    ReceiptLogCountDiff { got: usize, expected: usize },
    #[error("Receipt log is different.")]
    ReceiptLogDiff,
    #[error("Receipt log is different.")]
    ExecutionSuccessDiff { got: bool, expected: bool },
    #[error("Receipt root {got:?} is different than expected {expected:?}.")]
    ReceiptRootDiff { got: H256, expected: H256 },
    #[error("Header bloom filter {got:?} is different than expected {expected:?}.")]
    BloomLogDiff { got: Box<Bloom>, expected: Box<Bloom> },
    #[error("Transaction gas limit {transaction_gas_limit} is more than blocks available gas {block_available_gas}")]
    TransactionGasLimitMoreThenAvailableBlockGas {
        transaction_gas_limit: u64,
        block_available_gas: u64,
    },
    #[error("Block gas used {got} is different from expected gas used {expected}.")]
    BlockGasUsed { got: u64, expected: u64 },
    #[error("Provider error")]
    ProviderError,
}

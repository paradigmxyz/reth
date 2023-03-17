//! Commonly used errors for the `eth_` namespace.

use reth_primitives::{InvalidTransactionError, U256};
use revm::primitives::EVMError;

pub type RevmResult<T> = Result<T, RevmError>;

/// List of JSON-RPC error codes
#[derive(Debug, Copy, PartialEq, Eq, Clone)]
pub enum EthRpcErrorCode {
    /// Failed to send transaction, See also <https://github.com/MetaMask/eth-rpc-errors/blob/main/src/error-constants.ts>
    TransactionRejected,
    /// Custom geth error code, <https://github.com/vapory-legacy/wiki/blob/master/JSON-RPC-Error-Codes-Improvement-Proposal.md>
    ExecutionError,
    /// <https://eips.ethereum.org/EIPS/eip-1898>
    InvalidInput,
}

impl EthRpcErrorCode {
    /// Returns the error code as `i32`
    pub const fn code(&self) -> i32 {
        match *self {
            EthRpcErrorCode::TransactionRejected => -32003,
            EthRpcErrorCode::ExecutionError => 3,
            EthRpcErrorCode::InvalidInput => -32000,
        }
    }
}

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum RevmError {
    /// When a raw transaction is empty
    #[error("Empty transaction data")]
    EmptyRawTransactionData,
    #[error("Failed to decode signed transaction")]
    FailedToDecodeSignedTransaction,
    #[error("Invalid transaction signature")]
    InvalidTransactionSignature,
    #[error("PoolError")]
    PoolError,
    #[error("Unknown block number")]
    UnknownBlockNumber,
    #[error("Invalid block range")]
    InvalidBlockRange,
    /// An internal error where prevrandao is not set in the evm's environment
    #[error("Prevrandao not in th EVM's environment after merge")]
    PrevrandaoNotSet,
    #[error("Invalid transaction")]
    InvalidTransaction(#[from] InvalidTransactionError),
    #[error("Conflicting fee values in request. Both legacy gasPrice {gas_price} and maxFeePerGas {max_fee_per_gas} set")]
    ConflictingRequestGasPrice { gas_price: U256, max_fee_per_gas: U256 },
    #[error("Conflicting fee values in request. Both legacy gasPrice {gas_price} maxFeePerGas {max_fee_per_gas} and maxPriorityFeePerGas {max_priority_fee_per_gas} set")]
    ConflictingRequestGasPriceAndTipSet {
        gas_price: U256,
        max_fee_per_gas: U256,
        max_priority_fee_per_gas: U256,
    },
    #[error("Conflicting fee values in request. Legacy gasPrice {gas_price} and maxPriorityFeePerGas {max_priority_fee_per_gas} set")]
    RequestLegacyGasPriceAndTipSet { gas_price: U256, max_priority_fee_per_gas: U256 },
    #[error("Unknown error")]
    Internal,
}

impl<T> From<EVMError<T>> for RevmError
where
    T: Into<RevmError>,
{
    fn from(err: EVMError<T>) -> Self {
        match err {
            EVMError::Transaction(err) => InvalidTransactionError::from(err).into(),
            EVMError::PrevrandaoNotSet => RevmError::PrevrandaoNotSet,
            EVMError::Database(err) => err.into(),
        }
    }
}

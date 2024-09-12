//! RPC errors specific to OP.

use reth_evm_optimism::OptimismBlockExecutionError;
use reth_primitives::revm_primitives::{InvalidTransaction, OptimismInvalidTransaction};
use reth_rpc_eth_api::AsEthApiError;
use reth_rpc_eth_types::EthApiError;
use reth_rpc_server_types::result::{internal_rpc_err, rpc_err};
use reth_rpc_types::error::EthRpcErrorCode;

/// Optimism specific errors, that extend [`EthApiError`].
#[derive(Debug, thiserror::Error)]
pub enum OpEthApiError {
    /// L1 ethereum error.
    #[error(transparent)]
    Eth(#[from] EthApiError),
    /// EVM error originating from invalid optimism data.
    #[error(transparent)]
    Evm(#[from] OptimismBlockExecutionError),
    /// Thrown when calculating L1 gas fee.
    #[error("failed to calculate l1 gas fee")]
    L1BlockFeeError,
    /// Thrown when calculating L1 gas used
    #[error("failed to calculate l1 gas used")]
    L1BlockGasError,
    /// Wrapper for [`revm_primitives::InvalidTransaction`](InvalidTransaction).
    #[error(transparent)]
    InvalidTransaction(OptimismInvalidTransactionError),
}

impl AsEthApiError for OpEthApiError {
    fn as_err(&self) -> Option<&EthApiError> {
        match self {
            Self::Eth(err) => Some(err),
            _ => None,
        }
    }
}

impl From<OpEthApiError> for jsonrpsee_types::error::ErrorObject<'static> {
    fn from(err: OpEthApiError) -> Self {
        match err {
            OpEthApiError::Eth(err) => err.into(),
            OpEthApiError::InvalidTransaction(err) => err.into(),
            OpEthApiError::Evm(_) |
            OpEthApiError::L1BlockFeeError |
            OpEthApiError::L1BlockGasError => internal_rpc_err(err.to_string()),
        }
    }
}

/// Optimism specific invalid transaction errors
#[derive(thiserror::Error, Debug)]
pub enum OptimismInvalidTransactionError {
    /// A deposit transaction was submitted as a system transaction post-regolith.
    #[error("no system transactions allowed after regolith")]
    DepositSystemTxPostRegolith,
    /// A deposit transaction halted post-regolith
    #[error("deposit transaction halted after regolith")]
    HaltedDepositPostRegolith,
}

impl From<OptimismInvalidTransactionError> for jsonrpsee_types::error::ErrorObject<'static> {
    fn from(err: OptimismInvalidTransactionError) -> Self {
        match err {
            OptimismInvalidTransactionError::DepositSystemTxPostRegolith |
            OptimismInvalidTransactionError::HaltedDepositPostRegolith => {
                rpc_err(EthRpcErrorCode::TransactionRejected.code(), err.to_string(), None)
            }
        }
    }
}

impl TryFrom<InvalidTransaction> for OptimismInvalidTransactionError {
    type Error = InvalidTransaction;

    fn try_from(err: InvalidTransaction) -> Result<Self, Self::Error> {
        match err {
            InvalidTransaction::OptimismError(err) => match err {
                OptimismInvalidTransaction::DepositSystemTxPostRegolith => {
                    Ok(Self::DepositSystemTxPostRegolith)
                }
                OptimismInvalidTransaction::HaltedDepositPostRegolith => {
                    Ok(Self::HaltedDepositPostRegolith)
                }
            },
            _ => Err(err),
        }
    }
}

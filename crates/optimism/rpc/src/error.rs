//! RPC errors specific to OP.

use jsonrpsee::types::ErrorObject;
use reth_primitives::revm_primitives::{InvalidTransaction, OptimismInvalidTransaction};
use reth_rpc_eth_api::AsEthApiError;
use reth_rpc_eth_types::EthApiError;
use reth_rpc_server_types::result::{internal_rpc_err, rpc_err};
use reth_rpc_types::{error::EthRpcErrorCode, IntoRpcError};

/// Optimism specific errors, that extend [`EthApiError`].
#[derive(Debug, thiserror::Error)]
pub enum OpEthApiError {
    /// L1 ethereum error.
    #[error(transparent)]
    Eth(#[from] EthApiError),
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

impl IntoRpcError for OpEthApiError {
    fn into_rpc_err(self) -> ErrorObject<'static> {
        match self {
            Self::Eth(err) => err.into_rpc_err(),
            Self::L1BlockFeeError | Self::L1BlockGasError => internal_rpc_err(self.to_string()),
            Self::InvalidTransaction(err) => err.into_rpc_err(),
        }
    }
}

impl AsEthApiError for OpEthApiError {
    fn as_err(&self) -> Option<&EthApiError> {
        match self {
            Self::Eth(err) => Some(err),
            _ => None,
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

impl IntoRpcError for OptimismInvalidTransactionError {
    fn into_rpc_err(self) -> jsonrpsee_types::error::ErrorObject<'static> {
        match self {
            Self::DepositSystemTxPostRegolith | Self::HaltedDepositPostRegolith => {
                rpc_err(EthRpcErrorCode::TransactionRejected.code(), self.to_string(), None)
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

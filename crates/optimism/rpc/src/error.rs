//! RPC errors specific to OP.

use jsonrpsee::types::ErrorObject;
use reth_primitives::revm_primitives::InvalidTransaction;
use reth_rpc_eth_api::AsEthApiError;
use reth_rpc_eth_types::EthApiError;
use reth_rpc_server_types::result::internal_rpc_err;
use reth_rpc_types::ToRpcError;

/// Optimism specific errors, that extend [`EthApiError`].
#[derive(Debug, thiserror::Error)]
pub enum OpEthApiError {
    /// L1 ethereum error.
    #[error(transparent)]
    Core(#[from] EthApiError),
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

impl ToRpcError for OpEthApiError {
    fn to_rpc_error(&self) -> ErrorObject<'static> {
        match self {
            Self::L1BlockFeeError | Self::L1BlockGasError => internal_rpc_err(self.to_string()),
        }
    }
}

impl From<InvalidTransaction> for OpEthApiError {
    fn from(err: InvalidTransaction) -> Self {
        match err {
            InvalidTransaction::DepositSystemTxPostRegolith |
            InvalidTransaction::HaltedDepositPostRegolith => Self::InvalidTransaction(err.into()),
            _ => expect!("should cover all network specific match arms"),
        }
    }
}

impl AsEthApiError for OpEthApiError {
    fn as_err(&self) -> Option<&EthApiError> {
        match self {
            Self::Core(err) => Some(err),
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

impl ToRpcError for OptimismInvalidTransactionError {
    fn to_rpc_error(&self) -> jsonrpsee_types::error::ErrorObject<'static> {
        match self {
            Self::DepositSystemTxPostRegolith | Self::HaltedDepositPostRegolith => {
                rpc_err(EthRpcErrorCode::TransactionRejected.code(), self.to_string(), None)
            }
        }
    }
}

impl From<InvalidTransaction> for OptimismInvalidTransactionError {
    fn from(err: InvalidTransaction) -> Self {
        match err {
            InvalidTransaction::DepositSystemTxPostRegolith => Self::DepositSystemTxPostRegolith,
            InvalidTransaction::HaltedDepositPostRegolith => Self::HaltedDepositPostRegolith,
            _ => expect!("should cover all network specific match arms"),
        }
    }
}

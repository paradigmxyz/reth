//! RPC errors specific to OP.

use alloy_rpc_types::error::EthRpcErrorCode;
use jsonrpsee_types::error::INTERNAL_ERROR_CODE;
use reth_optimism_evm::OptimismBlockExecutionError;
use reth_primitives::revm_primitives::{InvalidTransaction, OptimismInvalidTransaction};
use reth_rpc_eth_api::AsEthApiError;
use reth_rpc_eth_types::EthApiError;
use reth_rpc_server_types::result::{internal_rpc_err, rpc_err};

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
    InvalidTransaction(#[from] OptimismInvalidTransactionError),
    /// Sequencer client error.
    #[error(transparent)]
    Sequencer(#[from] SequencerClientError),
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
            OpEthApiError::Sequencer(err) => err.into(),
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

/// Error type when interacting with the Sequencer
#[derive(Debug, thiserror::Error)]
pub enum SequencerClientError {
    /// Wrapper around an [`reqwest::Error`].
    #[error(transparent)]
    HttpError(#[from] reqwest::Error),
    /// Thrown when serializing transaction to forward to sequencer
    #[error("invalid sequencer transaction")]
    InvalidSequencerTransaction,
}

impl From<SequencerClientError> for jsonrpsee_types::error::ErrorObject<'static> {
    fn from(err: SequencerClientError) -> Self {
        jsonrpsee_types::error::ErrorObject::owned(
            INTERNAL_ERROR_CODE,
            err.to_string(),
            None::<String>,
        )
    }
}

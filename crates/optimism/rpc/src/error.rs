//! RPC errors specific to OP.

use alloy_rpc_types_eth::{error::EthRpcErrorCode, BlockError};
use jsonrpsee_types::error::{INTERNAL_ERROR_CODE, INVALID_PARAMS_CODE};
use reth_optimism_evm::OpBlockExecutionError;
use reth_rpc_eth_api::AsEthApiError;
use reth_rpc_eth_types::{error::api::FromEvmHalt, EthApiError};
use reth_rpc_server_types::result::{internal_rpc_err, rpc_err};
use revm::primitives::{EVMError, HaltReason, InvalidTransaction, OptimismInvalidTransaction};

/// Optimism specific errors, that extend [`EthApiError`].
#[derive(Debug, thiserror::Error)]
pub enum OpEthApiError {
    /// L1 ethereum error.
    #[error(transparent)]
    Eth(#[from] EthApiError),
    /// EVM error originating from invalid optimism data.
    #[error(transparent)]
    Evm(#[from] OpBlockExecutionError),
    /// Thrown when calculating L1 gas fee.
    #[error("failed to calculate l1 gas fee")]
    L1BlockFeeError,
    /// Thrown when calculating L1 gas used
    #[error("failed to calculate l1 gas used")]
    L1BlockGasError,
    /// Wrapper for [`revm_primitives::InvalidTransaction`](InvalidTransaction).
    #[error(transparent)]
    InvalidTransaction(#[from] OpInvalidTransactionError),
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
pub enum OpInvalidTransactionError {
    /// A deposit transaction was submitted as a system transaction post-regolith.
    #[error("no system transactions allowed after regolith")]
    DepositSystemTxPostRegolith,
    /// A deposit transaction halted post-regolith
    #[error("deposit transaction halted after regolith")]
    HaltedDepositPostRegolith,
    /// Transaction conditional errors.
    #[error(transparent)]
    TxConditionalErr(#[from] TxConditionalErr),
}

impl From<OpInvalidTransactionError> for jsonrpsee_types::error::ErrorObject<'static> {
    fn from(err: OpInvalidTransactionError) -> Self {
        match err {
            OpInvalidTransactionError::DepositSystemTxPostRegolith |
            OpInvalidTransactionError::HaltedDepositPostRegolith => {
                rpc_err(EthRpcErrorCode::TransactionRejected.code(), err.to_string(), None)
            }
            OpInvalidTransactionError::TxConditionalErr(_) => err.into(),
        }
    }
}

impl TryFrom<InvalidTransaction> for OpInvalidTransactionError {
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

/// Transaction conditional related error type
#[derive(Debug, thiserror::Error)]
pub enum TxConditionalErr {
    /// Transaction conditional cost exceeded maximum allowed
    #[error("conditional cost exceeded maximum allowed")]
    ConditionalCostExceeded,
    /// Invalid conditional parameters
    #[error("invalid conditional parameters")]
    InvalidCondition,
}

impl From<TxConditionalErr> for jsonrpsee_types::error::ErrorObject<'static> {
    fn from(err: TxConditionalErr) -> Self {
        jsonrpsee_types::error::ErrorObject::owned(
            INVALID_PARAMS_CODE,
            err.to_string(),
            None::<String>,
        )
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

impl From<BlockError> for OpEthApiError {
    fn from(error: BlockError) -> Self {
        Self::Eth(error.into())
    }
}

impl<DB> From<EVMError<DB>> for OpEthApiError
where
    EthApiError: From<EVMError<DB>>,
{
    fn from(error: EVMError<DB>) -> Self {
        Self::Eth(error.into())
    }
}

impl FromEvmHalt for OpEthApiError {
    fn from_evm_halt(halt: HaltReason, gas_limit: u64) -> Self {
        EthApiError::from_evm_halt(halt, gas_limit).into()
    }
}

//! RPC errors specific to Scroll.

use alloy_json_rpc::ErrorPayload;
use alloy_rpc_types_eth::BlockError;
use alloy_transport::{RpcError, TransportErrorKind};
use jsonrpsee_types::error::INTERNAL_ERROR_CODE;
use reth_evm::execute::ProviderError;
use reth_rpc_convert::transaction::EthTxEnvError;
use reth_rpc_eth_api::{AsEthApiError, TransactionConversionError};
use reth_rpc_eth_types::{error::api::FromEvmHalt, EthApiError};
use revm::context::result::{EVMError, HaltReason};

/// Scroll specific errors, that extend [`EthApiError`].
#[derive(Debug, thiserror::Error)]
pub enum ScrollEthApiError {
    /// L1 ethereum error.
    #[error(transparent)]
    Eth(#[from] EthApiError),
    /// Sequencer client error.
    #[error(transparent)]
    Sequencer(#[from] SequencerClientError),
}

impl AsEthApiError for ScrollEthApiError {
    fn as_err(&self) -> Option<&EthApiError> {
        match self {
            Self::Eth(err) => Some(err),
            _ => None,
        }
    }
}

impl From<ScrollEthApiError> for jsonrpsee_types::error::ErrorObject<'static> {
    fn from(err: ScrollEthApiError) -> Self {
        match err {
            ScrollEthApiError::Eth(err) => err.into(),
            ScrollEthApiError::Sequencer(err) => err.into(),
        }
    }
}

impl From<EthTxEnvError> for ScrollEthApiError {
    fn from(value: EthTxEnvError) -> Self {
        Self::Eth(EthApiError::from(value))
    }
}

impl From<BlockError> for ScrollEthApiError {
    fn from(error: BlockError) -> Self {
        Self::Eth(error.into())
    }
}

impl<DB> From<EVMError<DB>> for ScrollEthApiError
where
    EthApiError: From<EVMError<DB>>,
{
    fn from(error: EVMError<DB>) -> Self {
        Self::Eth(error.into())
    }
}

impl FromEvmHalt<HaltReason> for ScrollEthApiError {
    fn from_evm_halt(halt: HaltReason, gas_limit: u64) -> Self {
        EthApiError::from_evm_halt(halt, gas_limit).into()
    }
}

impl From<TransactionConversionError> for ScrollEthApiError {
    fn from(value: TransactionConversionError) -> Self {
        Self::Eth(EthApiError::from(value))
    }
}

impl From<ProviderError> for ScrollEthApiError {
    fn from(value: ProviderError) -> Self {
        Self::Eth(EthApiError::from(value))
    }
}

/// Error type when interacting with the Sequencer
#[derive(Debug, thiserror::Error)]
pub enum SequencerClientError {
    /// Wrapper around an [`RpcError<TransportErrorKind>`].
    #[error(transparent)]
    HttpError(#[from] RpcError<TransportErrorKind>),
    /// Thrown when serializing transaction to forward to sequencer
    #[error("invalid sequencer transaction")]
    InvalidSequencerTransaction,
}

impl From<SequencerClientError> for jsonrpsee_types::error::ErrorObject<'static> {
    fn from(err: SequencerClientError) -> Self {
        match err {
            SequencerClientError::HttpError(RpcError::ErrorResp(ErrorPayload {
                code,
                message,
                data,
            })) => jsonrpsee_types::error::ErrorObject::owned(code as i32, message, data),
            err => jsonrpsee_types::error::ErrorObject::owned(
                INTERNAL_ERROR_CODE,
                err.to_string(),
                None::<String>,
            ),
        }
    }
}

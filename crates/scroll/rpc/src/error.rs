//! RPC errors specific to Scroll.

use alloy_rpc_types_eth::BlockError;
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
}

impl AsEthApiError for ScrollEthApiError {
    fn as_err(&self) -> Option<&EthApiError> {
        match self {
            Self::Eth(err) => Some(err),
        }
    }
}

impl From<ScrollEthApiError> for jsonrpsee_types::error::ErrorObject<'static> {
    fn from(err: ScrollEthApiError) -> Self {
        match err {
            ScrollEthApiError::Eth(err) => err.into(),
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

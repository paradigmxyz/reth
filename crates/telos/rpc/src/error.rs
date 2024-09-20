//! RPC errors specific to Telos.

use reth_rpc_eth_api::AsEthApiError;
use reth_rpc_eth_types::EthApiError;

/// Telos specific errors, that extend [`EthApiError`].
#[derive(Debug, thiserror::Error)]
pub enum TelosEthApiError {
    #[error(transparent)]
    Eth(#[from] EthApiError),
    // #[error(transparent)]
    // TelosError(String),
}

impl AsEthApiError for TelosEthApiError {
    fn as_err(&self) -> Option<&EthApiError> {
        match self {
            Self::Eth(err) => Some(err),
        }
    }
}

impl From<TelosEthApiError> for jsonrpsee_types::error::ErrorObject<'static> {
    fn from(err: TelosEthApiError) -> Self {
        match err {
            TelosEthApiError::Eth(err) => err.into(),
            // TelosEthApiError::InvalidTransaction(err) => err.into(),
        }
    }
}

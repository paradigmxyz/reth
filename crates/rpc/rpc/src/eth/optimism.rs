//! Optimism specific types.

use crate::{
    eth::error::{EthApiError, ToRpcError},
    result::internal_rpc_err,
};
use jsonrpsee::types::ErrorObject;

/// Eth Optimism Api Error
#[cfg(feature = "optimism")]
#[derive(Debug, thiserror::Error)]
pub enum OptimismEthApiError {
    /// Wrapper around a [hyper::Error].
    #[error(transparent)]
    HyperError(#[from] hyper::Error),
    /// Wrapper around an [reqwest::Error].
    #[error(transparent)]
    HttpError(#[from] reqwest::Error),
    /// Thrown when serializing transaction to forward to sequencer
    #[error("invalid sequencer transaction")]
    InvalidSequencerTransaction,
    /// Thrown when calculating L1 gas fee
    #[error("failed to calculate l1 gas fee")]
    L1BlockFeeError,
    /// Thrown when calculating L1 gas used
    #[error("failed to calculate l1 gas used")]
    L1BlockGasError,
}

impl ToRpcError for OptimismEthApiError {
    fn to_rpc_error(&self) -> ErrorObject<'static> {
        match self {
            OptimismEthApiError::HyperError(err) => internal_rpc_err(err.to_string()),
            OptimismEthApiError::HttpError(err) => internal_rpc_err(err.to_string()),
            OptimismEthApiError::InvalidSequencerTransaction |
            OptimismEthApiError::L1BlockFeeError |
            OptimismEthApiError::L1BlockGasError => internal_rpc_err(self.to_string()),
        }
    }
}

impl From<OptimismEthApiError> for EthApiError {
    fn from(err: OptimismEthApiError) -> Self {
        EthApiError::other(err)
    }
}

//! Optimism specific types.

use jsonrpsee::types::ErrorObject;
use reth_rpc_types::ToRpcError;

use crate::{eth::error::EthApiError, result::internal_rpc_err};

/// Eth Optimism Api Error
#[cfg(feature = "optimism")]
#[derive(Debug, thiserror::Error)]
pub enum OptimismEthApiError {
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
            OptimismEthApiError::L1BlockFeeError | OptimismEthApiError::L1BlockGasError => {
                internal_rpc_err(self.to_string())
            }
        }
    }
}

impl From<OptimismEthApiError> for EthApiError {
    fn from(err: OptimismEthApiError) -> Self {
        EthApiError::other(err)
    }
}

//! Loads and formats OP transaction RPC response.   

use jsonrpsee_types::error::ErrorObject;
use reth_rpc_eth_types::EthApiError;
use reth_rpc_server_types::result::internal_rpc_err;
use reth_rpc_types::ToRpcError;

/// Optimism specific errors, that extend [`EthApiError`].
#[derive(Debug, thiserror::Error)]
pub enum OptimismEthApiError {
    /// Thrown when calculating L1 gas fee.
    #[error("failed to calculate l1 gas fee")]
    L1BlockFeeError,
    /// Thrown when calculating L1 gas used
    #[error("failed to calculate l1 gas used")]
    L1BlockGasError,
}

impl ToRpcError for OptimismEthApiError {
    fn to_rpc_error(&self) -> ErrorObject<'static> {
        match self {
            Self::L1BlockFeeError | Self::L1BlockGasError => internal_rpc_err(self.to_string()),
        }
    }
}

impl From<OptimismEthApiError> for EthApiError {
    fn from(err: OptimismEthApiError) -> Self {
        Self::other(err)
    }
}

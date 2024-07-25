//! Trait for specifying `eth` API types that may be network dependent.

use reth_rpc_types::IntoRpcError;

use crate::{AsEthApiError, FromEthApiError, FromEvmError};

/// Network specific `eth` API types.
pub trait EthApiTypes: Send + Sync {
    /// Extension of [`EthApiError`], with network specific errors.
    type Error: IntoRpcError + FromEthApiError + AsEthApiError + FromEvmError;
}

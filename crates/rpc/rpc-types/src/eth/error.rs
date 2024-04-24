//! Implementation specific Errors for the `eth_` namespace.

use jsonrpsee_types::ErrorObject;

/// A tait for custom rpc errors used by Other variants of rpc errors.
pub trait ToRpcError: std::error::Error + Send + Sync + 'static {
    /// Converts the error to a JSON-RPC error object.
    fn to_rpc_error(&self) -> ErrorObject<'static>;
}

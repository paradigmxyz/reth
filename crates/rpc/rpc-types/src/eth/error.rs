//! Implementation specific Errors for the `eth_` namespace.

use jsonrpsee_types::ErrorObject;

/// A trait to convert an error to an RPC error.
pub trait ToRpcError: std::error::Error + Send + Sync + 'static {
    /// Converts the error to a JSON-RPC error object.
    fn to_rpc_error(&self) -> ErrorObject<'static>;
}

/// A trait to convert an error to an RPC error.
pub trait IntoRpcError: std::error::Error + Send + Sync + 'static {
    /// Converts the error to a JSON-RPC error object.
    fn into_rpc_err(self) -> ErrorObject<'static>;
}

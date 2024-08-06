//! Implementation specific Errors for the `eth_` namespace.

/// A trait to convert an error to an RPC error.
#[cfg(feature = "default")]
pub trait ToRpcError: std::error::Error + Send + Sync + 'static {
    /// Converts the error to a JSON-RPC error object.
    fn to_rpc_error(&self) -> jsonrpsee_types::ErrorObject<'static>;
}

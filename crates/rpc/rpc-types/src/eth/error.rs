//! Implementation specific Errors for the `eth_` namespace.

/// A trait to convert an error to an RPC error.
#[cfg(feature = "jsonrpsee-types")]
pub trait ToRpcError: core::error::Error + Send + Sync + 'static {
    /// Converts the error to a JSON-RPC error object.
    fn to_rpc_error(&self) -> jsonrpsee_types::ErrorObject<'static>;
}

#[cfg(feature = "jsonrpsee-types")]
impl ToRpcError for jsonrpsee_types::ErrorObject<'static> {
    fn to_rpc_error(&self) -> jsonrpsee_types::ErrorObject<'static> {
        self.clone()
    }
}

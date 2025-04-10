/// The error type for the Scroll engine API.
#[derive(thiserror::Error, Debug)]
pub enum ScrollEngineApiError {
    /// Error when decoding a response from an rpsee client.
    #[error("Jsonrpsee error: {0}")]
    JsonRpseeError(#[from] jsonrpsee::core::ClientError),
    /// Error when decoding a response from an alloy client.
    #[error("Alloy error: {0}")]
    AlloyError(#[from] alloy_transport::TransportError),
}

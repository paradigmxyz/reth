//! Commonly used errors for the `engine_` namespace.

/// List of Engine API errors, see <https://github.com/ethereum/execution-apis/blob/main/src/engine/common.md#errors>
#[derive(Debug, Copy, PartialEq, Eq, Clone, thiserror::Error)]
pub enum EngineRpcError {
    /// Invalid JSON was received by the server.
    #[error("Invalid JSON was received by the server")]
    ParseError,
    /// The JSON sent is not a valid Request object.
    #[error("The JSON sent is not a valid Request object")]
    InvalidRequest,
    /// The method does not exist / is not available.
    #[error("The method does not exist / is not available")]
    MethodNotFound,
    /// Invalid method parameter(s).
    #[error("Invalid method parameter(s)")]
    InvalidParams,
    /// Internal JSON-RPC error.
    #[error("Internal JSON-RPC error")]
    InternalError,
    /// Generic client error while processing request.
    #[error("Server error")]
    ServerError,
    /// Payload does not exist / is not available.
    #[error("Unknown payload")]
    UnknownPayload,
    /// Forkchoice state is invalid / inconsistent.
    #[error("Invalid forkchoice state")]
    InvalidForkchoiceState,
    /// Payload attributes are invalid / inconsistent.
    #[error("Invalid payload attributes")]
    InvalidPayloadAttributes,
    /// Number of requested entities is too large.
    #[error("Too large request")]
    TooLargeRequest,
}

impl EngineRpcError {
    /// Returns the error code as `i32`
    pub const fn code(&self) -> i32 {
        match *self {
            EngineRpcError::ParseError => -32700,
            EngineRpcError::InvalidRequest => -32600,
            EngineRpcError::MethodNotFound => -32601,
            EngineRpcError::InvalidParams => -32602,
            EngineRpcError::InternalError => -32603,
            EngineRpcError::ServerError => -32000,
            EngineRpcError::UnknownPayload => -38001,
            EngineRpcError::InvalidForkchoiceState => -38002,
            EngineRpcError::InvalidPayloadAttributes => -38003,
            EngineRpcError::TooLargeRequest => -38004,
        }
    }
}

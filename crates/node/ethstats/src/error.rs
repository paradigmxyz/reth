use thiserror::Error;

/// Errors that can occur during `WebSocket` connection handling
#[derive(Debug, Error)]
pub enum ConnectionError {
    /// The `WebSocket` connection was closed unexpectedly
    #[error("Connection closed")]
    ConnectionClosed,

    /// Error occurred during JSON serialization/deserialization
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// Error occurred during `WebSocket` communication
    #[error("WebSocket error: {0}")]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),
}

/// Main error type for the `EthStats` client
///
/// This enum covers all possible errors that can occur when interacting
/// with an `EthStats` server, including connection issues, authentication
/// problems, data fetching errors, and various I/O operations.
#[derive(Debug, Error)]
pub enum EthStatsError {
    /// The provided URL is invalid or malformed
    #[error("Invalid URL: {0}")]
    InvalidUrl(String),

    /// Error occurred during connection establishment or maintenance
    #[error("Connection error: {0}")]
    ConnectionError(#[from] ConnectionError),

    /// Authentication failed with the `EthStats` server
    #[error("Authentication error: {0}")]
    AuthError(String),

    /// Attempted to perform an operation while not connected to the server
    #[error("Not connected to server")]
    NotConnected,

    /// Error occurred during JSON serialization or deserialization
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// Error occurred during `WebSocket` communication
    #[error("WebSocket error: {0}")]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),

    /// Operation timed out
    #[error("Timeout error")]
    Timeout,

    /// Error occurred while parsing a URL
    #[error("URL parsing error: {0}")]
    Url(#[from] url::ParseError),

    /// Requested block was not found in the blockchain
    #[error("Block not found: {0}")]
    BlockNotFound(u64),

    /// Error occurred while fetching data from the blockchain or server
    #[error("Data fetch error: {0}")]
    DataFetchError(String),

    /// The request sent to the server was invalid or malformed
    #[error("Inivalid request")]
    InvalidRequest,
}

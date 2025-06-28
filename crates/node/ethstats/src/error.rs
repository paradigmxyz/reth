use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConnectionError {
    #[error("Connection closed")]
    ConnectionClosed,
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("WebSocket error: {0}")]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),
}

#[derive(Debug, Error)]
pub enum EthStatsError {
    #[error("Invalid URL: {0}")]
    InvalidUrl(String),

    #[error("Connection error: {0}")]
    ConnectionError(#[from] ConnectionError),

    #[error("Authentication error: {0}")]
    AuthError(String),

    #[error("Not connected to server")]
    NotConnected,

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("WebSocket error: {0}")]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),

    #[error("Timeout error")]
    Timeout,

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("URL parsing error: {0}")]
    Url(#[from] url::ParseError),

    #[error("Block not found: {0}")]
    BlockNotFound(u64),

    #[error("Data fetch error: {0}")]
    DataFetchError(String),

    #[error("Inivalid request")]
    InvalidRequest,
}

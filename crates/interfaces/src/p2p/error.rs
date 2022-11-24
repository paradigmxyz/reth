use tokio::sync::{mpsc, oneshot};

/// Result alias for result of a request.
pub type RequestResult<T> = Result<T, RequestError>;

/// Error variants that can happen when sending requests to a session.
#[derive(Debug, thiserror::Error, Clone)]
#[allow(missing_docs)]
pub enum RequestError {
    #[error("Closed channel to the peer.")]
    ChannelClosed,
    #[error("Not connected to the peer.")]
    NotConnected,
    #[error("Connection to a peer dropped while handling the request.")]
    ConnectionDropped,
    #[error("Capability Message is not supported by remote peer.")]
    UnsupportedCapability,
    #[error("Request timed out while awaiting response.")]
    Timeout,
    #[error("Received bad response.")]
    BadResponse,
}

impl<T> From<mpsc::error::SendError<T>> for RequestError {
    fn from(_: mpsc::error::SendError<T>) -> Self {
        RequestError::ChannelClosed
    }
}

impl From<oneshot::error::RecvError> for RequestError {
    fn from(_: oneshot::error::RecvError) -> Self {
        RequestError::ChannelClosed
    }
}

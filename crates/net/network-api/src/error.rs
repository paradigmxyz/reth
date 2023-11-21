use thiserror::Error;
use tokio::sync::{mpsc, oneshot};

/// Network Errors
#[allow(missing_docs)]
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum NetworkError {
    #[error("sender has been dropped")]
    ChannelClosed,
}

impl<T> From<mpsc::error::SendError<T>> for NetworkError {
    fn from(_: mpsc::error::SendError<T>) -> Self {
        NetworkError::ChannelClosed
    }
}

impl From<oneshot::error::RecvError> for NetworkError {
    fn from(_: oneshot::error::RecvError) -> Self {
        NetworkError::ChannelClosed
    }
}

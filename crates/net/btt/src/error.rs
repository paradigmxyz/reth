//! Errors for this crate

use crate::peer::error::PeerError;
use std::io;
use tokio::sync::{mpsc::error::SendError, oneshot::error::RecvError};

/// Error alias for this crate
pub type TorrentResult<T> = std::result::Result<T, Error>;

/// Bundles various error cases that can happen in this crate.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The channel on which some component in engine was listening or sending
    /// died.
    #[error("channel error")]
    Channel,
    /// IO-related error.
    #[error(transparent)]
    Io(#[from] io::Error),
    /// The torrent ID did not correspond to any entry. This is returned when
    /// the user specified a torrent that does not exist.
    #[error("invalid torrent id")]
    InvalidTorrentId,
}

impl<T> From<SendError<T>> for Error {
    fn from(_: SendError<T>) -> Self {
        Self::Channel
    }
}

impl From<RecvError> for Error {
    fn from(_: RecvError) -> Self {
        Self::Channel
    }
}

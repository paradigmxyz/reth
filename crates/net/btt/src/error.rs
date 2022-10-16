//! Errors for this crate

use crate::{
    peer::error::{IoError, PeerError},
    torrent::TorrentId,
    tracker::TrackerError,
};
use std::{fmt, fmt::Display, io, net::SocketAddr};
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
    /// The torrent download location is not valid.
    #[error("invalid download path")]
    InvalidDownloadPath,
    /// An error specific to a torrent.
    #[error("torrent {id} error: {msg}")]
    Torrent { id: TorrentId, msg: String },
    /// An error that occurred while a torrent was announcing to tracker.
    #[error("torrent {id} tracker error {error}")]
    Tracker { id: TorrentId, error: TrackerError },
    /// An error that occurred in a torrent's session with a peer.
    #[error("torrent {id} peer {addr} error: {error}")]
    Peer { id: TorrentId, addr: SocketAddr, error: PeerError },
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

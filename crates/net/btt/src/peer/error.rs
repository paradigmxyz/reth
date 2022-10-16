use std::fmt;

pub use tokio::{io::Error as IoError, sync::mpsc::error::SendError};

pub(crate) type PeerResult<T> = std::result::Result<T, PeerError>;

/// Error type returned on failed peer sessions.
///
/// This error is non-fatal so it should not be grouped with the global `Error`
/// type as it may be recovered from.
#[derive(Debug, thiserror::Error)]
pub enum PeerError {
    /// The bitfield message was not sent after the handshake. According to the
    /// protocol, it should only be accepted after the handshake and when
    /// received at any other time, connection is severed.
    #[error("received unexpected bitfield")]
    BitfieldNotAfterHandshake,
    /// The channel on which some component in engine was listening or sending
    /// died.
    #[error("channel error")]
    Channel,
    /// Peers are not allowed to request blocks while they are choked. If they
    /// do so, their connection is severed.
    #[error("choked peer sent request")]
    RequestWhileChoked,
    /// A peer session timed out because neither side of the connection became
    /// interested in each other.
    #[error("inactivity timeout")]
    InactivityTimeout,
    /// The block information the peer sent is invalid.
    #[error("invalid block info")]
    InvalidBlockInfo,
    /// The block's piece index is invalid.
    #[error("invalid piece index")]
    InvalidPieceIndex,
    /// Peer's torrent info hash did not match ours.
    #[error("invalid info hash")]
    InvalidInfoHash,
    /// Peer's torrent info hash did not match ours.
    #[error("invalid handshake")]
    InvalidHandshake,
    /// An IO error ocurred.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl<T> From<SendError<T>> for PeerError {
    fn from(_: SendError<T>) -> Self {
        Self::Channel
    }
}

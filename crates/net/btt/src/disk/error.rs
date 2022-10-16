use crate::error::Error;
use std::fmt;

/// The disk IO result type.
pub(crate) type Result<T, E = Error> = std::result::Result<T, E>;

/// Error type returned on failed torrent allocations.
///
/// This error is non-fatal so it should not be grouped with the global `Error`
/// type as it may be recovered from.
#[derive(Debug, thiserror::Error)]
pub(crate) enum NewTorrentError {
    /// The torrent entry already exists in `Disk`'s hashmap of torrents.
    #[error("disk torrent entry already exists")]
    AlreadyExists,
    /// IO error while allocating torrent.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Error type returned on failed block writes.
///
/// This error is non-fatal so it should not be grouped with the global `Error`
/// type as it may be recovered from.
#[derive(Debug)]
pub(crate) enum WriteError {
    /// An IO error ocurred.
    Io(std::io::Error),
}

impl fmt::Display for WriteError {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(fmt, "{}", e),
        }
    }
}

/// Error type returned on failed block reads.
///
/// This error is non-fatal so it should not be grouped with the global `Error`
/// type as it may be recovered from.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ReadError {
    /// The block's offset in piece is invalid.
    #[error("invalid block offset")]
    InvalidBlockOffset,
    /// The block is valid within torrent but its data has not been downloaded
    /// yet or has been deleted.
    #[error("torrent data missing")]
    MissingData,
    /// An IO error occurred.
    #[error(transparent)]
    Io(std::io::Error),
}

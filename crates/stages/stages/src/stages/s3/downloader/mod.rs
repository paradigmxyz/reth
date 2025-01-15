//! Provides functionality for downloading files in chunks from a remote source. It supports
//! concurrent downloads, resuming interrupted downloads, and file integrity verification.

mod error;
mod fetch;
mod meta;
mod worker;

pub(crate) use error::DownloaderError;
pub use fetch::fetch;
pub use meta::Metadata;

/// Response sent by the fetch task to `S3Stage` once it has downloaded all files of a block
/// range.
pub(crate) enum S3DownloaderResponse {
    /// A new block range was downloaded.
    AddedNewRange,
    /// The last requested block range was downloaded.
    Done,
}

impl S3DownloaderResponse {
    /// Whether the downloaded block range is the last requested one.
    pub(crate) const fn is_done(&self) -> bool {
        matches!(self, Self::Done)
    }
}

/// Chunk nth remaining range to be downloaded.
#[derive(Debug)]
pub struct RemainingChunkRange {
    /// The nth chunk
    pub index: usize,
    /// Start of range
    pub start: usize,
    /// End of range
    pub end: usize,
}

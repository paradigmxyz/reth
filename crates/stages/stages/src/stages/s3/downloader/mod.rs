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
pub(crate) struct S3DownloaderResponse {
    /// Downloaded range.
    pub range: std::ops::RangeInclusive<u64>,
    /// Whether the fetch task has downloaded the last requested block range.
    pub is_done: bool,
}

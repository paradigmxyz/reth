//! Plumbing shared by the account, storage and bytecode downloads.
//!
//! [`DownloadContext`] carries the client, database and request settings each download sends
//! and commits through; [`SnapRecord`] stores their progress as versioned metadata records.

mod download;
mod record;

pub(crate) use download::DownloadContext;
pub use download::{DEFAULT_RESPONSE_BYTES, MAX_HASH};
pub(crate) use record::SnapRecord;

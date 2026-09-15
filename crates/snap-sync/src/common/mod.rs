//! Download contexts, request retries and versioned progress records shared across sync phases.

mod download;
mod record;

pub(crate) use download::{push_peer, request_options, DownloadContext, SnapRequests};
pub use download::{DEFAULT_RESPONSE_BYTES, MAX_HASH};
pub(crate) use record::SnapRecord;

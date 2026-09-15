//! Plumbing shared by the account, storage, bytecode and block access list downloads.
//!
//! [`DownloadContext`] and [`SnapRequests`] carry the client, runtime and request ids each
//! download sends through; [`read_record`] and [`write_record`] store versioned progress.

mod download;
mod record;

pub(crate) use download::{push_peer, request_options, DownloadContext, SnapRequests};
pub use download::{DEFAULT_RESPONSE_BYTES, MAX_HASH};
pub(crate) use record::{read_record, write_record};

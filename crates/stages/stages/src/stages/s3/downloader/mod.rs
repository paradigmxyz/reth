//! Provides functionality for downloading files in chunks from a remote source. It supports
//! concurrent downloads, resuming interrupted downloads, and file integrity verification.

mod error;
mod fetch;
mod meta;
mod worker;

pub use fetch::fetch;
pub use meta::Metadata;

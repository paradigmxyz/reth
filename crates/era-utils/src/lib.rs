//! Utilities to store history from downloaded ERA files with storage-api
//!
//! The import is downloaded using [`reth_era_downloader`] and parsed using [`reth_era`].

mod history;

/// Imports history from ERA files.
pub use history::import;

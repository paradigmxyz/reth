//! Utilities to store history from downloaded ERA files with storage-api
//!  and export it to recreate era1 files.

/// The import is downloaded using [`reth_era_downloader`] and parsed using [`reth_era`].
mod import;

/// Export block history data from the database to recreate era1 files.
mod export;

/// Imports history from ERA files.
pub use import::import;

/// Export history from storage-api between 2 blocks
/// with parameters defined in [`ExportConfig`].
pub use export::{export, ExportConfig};

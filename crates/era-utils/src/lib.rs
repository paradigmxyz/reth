//! Utilities to store history from downloaded ERA files with storage-api
//!  and export it to recreate era1 files.
//!
//! The import is downloaded using [`reth_era_downloader`] and parsed using [`reth_era`].

mod history;

/// Export block history data from the database to recreate era1 files.
mod export;

/// Export history from storage-api between 2 blocks
/// with parameters defined in [`ExportConfig`].
pub use export::{export, ExportConfig};

/// Imports history from ERA files.
pub use history::{
    build_index, decode, import, open, process, process_iter, save_stage_checkpoints, ProcessIter,
};

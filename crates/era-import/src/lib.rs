//! Imports history from ERA files.
//!
//! The import is downloaded using [`reth_era_downloader`] and parsed using [`reth_era`].

mod history;

pub use history::import;

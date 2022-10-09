//! Errors for this crate

use std::io;

/// Bundles various error cases that can happen in this crate.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// IO-related error.
    #[error(transparent)]
    Io(#[from] io::Error),
}

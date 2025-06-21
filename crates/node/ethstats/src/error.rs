//! Error types for the ethstats service

use thiserror::Error;

/// Errors that can occur in the ethstats service
#[derive(Debug, Error)]
pub enum Error {
    /// Invalid URL format
    #[error("Invalid URL format")]
    InvalidUrl,

    /// Invalid port number
    #[error("Invalid port number")]
    InvalidPort,
}


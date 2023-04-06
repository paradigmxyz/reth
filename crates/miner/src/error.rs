//! Error types emitted by types or implementations of this crate.

/// Possible error variants during payload building.
#[derive(Debug, thiserror::Error)]
#[error("Payload error")]
pub struct PayloadError;

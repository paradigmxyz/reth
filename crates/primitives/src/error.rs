//! Primitive errors
use thiserror::Error;

/// Primitives error type.
#[derive(Debug, Error)]
pub enum Error {
    /// The provided input is invalid.
    #[error("The provided input is invalid.")]
    InvalidInput,
    /// Failed to deserialize data into type.
    #[error("Failed to deserialize data into type.")]
    FailedDeserialize,
}

//! Primitive errors
use thiserror::Error;

/// Primitives error type.
#[derive(Debug, Error)]
pub enum Error {
    /// Input provided is invalid.
    #[error("Input provided is invalid.")]
    InvalidInput,
    /// Failed to deserialize data into type.
    #[error("Failed to deserialize data into type.")]
    FailedDeserialize,
}

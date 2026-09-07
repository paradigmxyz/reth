//! Failures raised while assembling a snap state generation.

/// Error returned while assembling a snap state generation.
#[derive(Debug, thiserror::Error)]
pub enum SnapSyncError {
    /// The provider rejected a database operation.
    #[error("snap database operation failed: {0}")]
    Database(String),
}

// Erasing provider error types keeps the session's public bounds small.
pub(crate) fn db_error(error: impl core::fmt::Display) -> SnapSyncError {
    SnapSyncError::Database(error.to_string())
}

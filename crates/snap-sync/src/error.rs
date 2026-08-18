//! Reports failures that make a partial Snap generation unsafe to resume.
//!
//! Layout, phase, and marker errors remain distinct from provider failures for retry decisions.

use crate::SnapPhase;
use alloy_primitives::B256;

/// Error returned while assembling a Snap state generation.
#[derive(Debug, thiserror::Error)]
pub enum SnapSyncError {
    /// The database uses plain canonical state, which hashed Snap keys cannot populate.
    #[error("snap sync requires the v2 hashed-state layout")]
    UnsupportedStorageLayout,
    /// A generation operation was attempted in the wrong phase.
    #[error("snap generation is in {actual:?}, expected {expected:?}")]
    UnexpectedPhase {
        /// Required phase.
        expected: SnapPhase,
        /// Persisted phase.
        actual: SnapPhase,
    },
    /// A range completed after another operation advanced the generation.
    #[error("snap generation changed while a range was in flight")]
    StaleGeneration,
    /// A continuation must move past the committed range.
    #[error("snap account cursor {next} does not advance {current}")]
    NonAdvancingAccountCursor {
        /// Persisted inclusive origin.
        current: B256,
        /// Proposed inclusive origin.
        next: B256,
    },
    /// The persisted generation marker cannot be resumed safely.
    #[error("invalid snap generation marker: {0}")]
    InvalidGeneration(String),
    /// The provider rejected a database operation.
    #[error("snap database operation failed: {0}")]
    Database(String),
}

// Erasing provider error types keeps the coordinator's public bounds small.
pub(crate) fn db_error(error: impl core::fmt::Display) -> SnapSyncError {
    SnapSyncError::Database(error.to_string())
}

//! Failures raised while assembling a snap state generation.

use reth_storage_api::SnapAttemptId;
use reth_storage_errors::provider::ProviderError;

/// Error returned while assembling a snap state generation.
#[derive(Debug, thiserror::Error)]
pub enum SnapSyncError {
    /// A header lookup failed.
    #[error(transparent)]
    Provider(#[from] ProviderError),
    /// The storage layout keys state by address, which snap cannot fill in without preimages.
    #[error("snap synchronization requires the hashed state layout")]
    UnsupportedStorage,
    /// No attempt owns the persisted state.
    #[error("no snap attempt owns the persisted state")]
    NoAttempt,
    /// The database already holds an attempt's state, which a new attempt would adopt.
    #[error("database already holds state of snap attempt {attempt}")]
    ExistingAttempt {
        /// Attempt that wrote the state.
        attempt: SnapAttemptId,
    },
    /// A write was presented for an attempt or pivot that no longer owns the persisted state.
    #[error("stale snap write for attempt {attempt} at state version {state_version}")]
    StaleWrite {
        /// Attempt the rejected write claims.
        attempt: SnapAttemptId,
        /// State version the rejected write was proved against.
        state_version: u64,
    },
}

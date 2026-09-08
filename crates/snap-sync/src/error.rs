//! Failures raised while assembling a snap state generation.

use reth_storage_errors::provider::ProviderError;

/// Error returned while assembling a snap state generation.
#[derive(Debug, thiserror::Error)]
pub enum SnapSyncError {
    /// A header lookup failed.
    #[error(transparent)]
    Provider(#[from] ProviderError),
}

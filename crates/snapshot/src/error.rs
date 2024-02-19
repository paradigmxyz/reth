use reth_db::DatabaseError;
use reth_interfaces::RethError;
use reth_provider::ProviderError;
use thiserror::Error;

/// Error returned by [crate::Snapshotter::run]
#[derive(Error, Debug)]
/// Errors that can occur during snapshotting.
pub enum SnapshotterError {
    /// Inconsistent data error.
    #[error("inconsistent data: {0}")]
    InconsistentData(&'static str),

    /// Error related to the interface.
    #[error(transparent)]
    Interface(#[from] RethError),

    /// Error related to the database.
    #[error(transparent)]
    Database(#[from] DatabaseError),

    /// Error related to the provider.
    #[error(transparent)]
    Provider(#[from] ProviderError),
}

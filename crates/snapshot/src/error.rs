use reth_db::DatabaseError;
use reth_interfaces::RethError;
use reth_provider::ProviderError;
use thiserror::Error;

/// Error returned by [crate::Snapshotter::run]
#[derive(Error, Debug)]
#[allow(missing_docs)]
pub enum SnapshotterError {
    #[error("Inconsistent data: {0}")]
    InconsistentData(&'static str),

    #[error("An interface error occurred.")]
    Interface(#[from] RethError),

    #[error(transparent)]
    Database(#[from] DatabaseError),

    #[error(transparent)]
    Provider(#[from] ProviderError),
}

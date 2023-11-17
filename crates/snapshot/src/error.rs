use reth_db::DatabaseError;
use reth_interfaces::RethError;
use reth_provider::ProviderError;
use thiserror::Error;

/// Error returned by [crate::Snapshotter::run]
#[derive(Error, Debug)]
#[allow(missing_docs)]
pub enum SnapshotterError {
    #[error("inconsistent data: {0}")]
    InconsistentData(&'static str),

    #[error(transparent)]
    Interface(#[from] RethError),

    #[error(transparent)]
    Database(#[from] DatabaseError),

    #[error(transparent)]
    Provider(#[from] ProviderError),
}

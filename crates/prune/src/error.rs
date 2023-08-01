use reth_db::DatabaseError;
use reth_provider::ProviderError;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum PrunerError {
    #[error("Inconsistent data: {0}")]
    InconsistentData(&'static str),

    #[error("An interface error occurred.")]
    Interface(#[from] reth_interfaces::Error),

    #[error(transparent)]
    Database(#[from] DatabaseError),

    #[error(transparent)]
    Provider(#[from] ProviderError),
}

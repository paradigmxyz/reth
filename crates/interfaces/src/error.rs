/// Result alias for `Error`
pub type Result<T> = std::result::Result<T, Error>;

/// Core error variants possible when interacting with the blockchain
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum Error {
    #[error(transparent)]
    Execution(#[from] crate::executor::Error),

    #[error(transparent)]
    Consensus(#[from] crate::consensus::Error),

    #[error(transparent)]
    Database(#[from] crate::db::Error),

    #[error(transparent)]
    Provider(#[from] crate::provider::Error),

    #[error(transparent)]
    Network(#[from] reth_network_api::NetworkError),
}

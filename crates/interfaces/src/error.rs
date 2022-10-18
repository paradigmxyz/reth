/// Result alias for `Error`
pub type Result<T> = std::result::Result<T, Error>;

/// Core error variants possible when interacting with the blockchain
#[derive(Debug, thiserror::Error)]
#[allow(missing_docs)]
pub enum Error {
    #[error(transparent)]
    Execution(#[from] crate::executor::Error),

    #[error(transparent)]
    Consensus(#[from] crate::consensus::Error),
}

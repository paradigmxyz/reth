/// Result alias for [`RethError`].
pub type RethResult<T> = Result<T, RethError>;

/// Core error variants possible when interacting with the blockchain.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum RethError {
    #[error(transparent)]
    Execution(#[from] crate::executor::BlockExecutionError),

    #[error(transparent)]
    Consensus(#[from] crate::consensus::ConsensusError),

    #[error(transparent)]
    Database(#[from] crate::db::DatabaseError),

    #[error(transparent)]
    Provider(#[from] crate::provider::ProviderError),

    #[error(transparent)]
    Network(#[from] reth_network_api::NetworkError),

    #[error(transparent)]
    Canonical(#[from] crate::blockchain_tree::error::CanonicalError),

    #[error("{0}")]
    Custom(String),
}

impl From<crate::blockchain_tree::error::BlockchainTreeError> for RethError {
    fn from(error: crate::blockchain_tree::error::BlockchainTreeError) -> Self {
        RethError::Canonical(error.into())
    }
}

impl From<reth_primitives::fs::FsPathError> for RethError {
    fn from(err: reth_primitives::fs::FsPathError) -> Self {
        RethError::Custom(err.to_string())
    }
}

// We don't want these types to be too large because they're used in a lot of places.
const _SIZE_ASSERTIONS: () = {
    // Main error.
    let _: [(); 64] = [(); std::mem::size_of::<RethError>()];

    // Biggest variant.
    let _: [(); 64] = [(); std::mem::size_of::<crate::provider::ProviderError>()];

    // Other common types.
    let _: [(); 16] = [(); std::mem::size_of::<crate::db::DatabaseError>()];
};

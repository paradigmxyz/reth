use crate::{
    blockchain_tree::error::{BlockchainTreeError, CanonicalError},
    consensus::ConsensusError,
    db::DatabaseError,
    executor::BlockExecutionError,
    provider::ProviderError,
};
use reth_network_api::NetworkError;
use reth_primitives::fs::FsPathError;

/// Result alias for [`RethError`].
pub type RethResult<T> = Result<T, RethError>;

/// Core error variants possible when interacting with the blockchain.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum RethError {
    #[error(transparent)]
    Execution(#[from] BlockExecutionError),

    #[error(transparent)]
    Consensus(#[from] ConsensusError),

    #[error(transparent)]
    Database(#[from] DatabaseError),

    #[error(transparent)]
    Provider(#[from] ProviderError),

    #[error(transparent)]
    Network(#[from] NetworkError),

    #[error(transparent)]
    Canonical(#[from] CanonicalError),

    #[error("{0}")]
    Custom(String),
}

impl From<BlockchainTreeError> for RethError {
    fn from(error: BlockchainTreeError) -> Self {
        RethError::Canonical(CanonicalError::BlockchainTree(error))
    }
}

impl From<FsPathError> for RethError {
    fn from(err: FsPathError) -> Self {
        RethError::Custom(err.to_string())
    }
}

// Some types are used a lot. Make sure they don't unintentionally get bigger.
#[cfg(all(target_arch = "x86_64", target_pointer_width = "64"))]
mod size_asserts {
    use super::*;

    macro_rules! static_assert_size {
        ($t:ty, $sz:expr) => {
            const _: [(); $sz] = [(); std::mem::size_of::<$t>()];
        };
    }

    static_assert_size!(RethError, 56);
    static_assert_size!(BlockExecutionError, 48);
    static_assert_size!(ConsensusError, 48);
    static_assert_size!(DatabaseError, 16);
    static_assert_size!(ProviderError, 48);
    static_assert_size!(NetworkError, 0);
    static_assert_size!(CanonicalError, 48);
}

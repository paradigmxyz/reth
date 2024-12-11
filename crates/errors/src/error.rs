use reth_blockchain_tree_api::error::{BlockchainTreeError, CanonicalError};
use reth_consensus::ConsensusError;
use reth_execution_errors::BlockExecutionError;
use reth_fs_util::FsPathError;
use reth_storage_errors::{db::DatabaseError, provider::ProviderError};
use std::fmt::Display;

/// Result alias for [`RethError`].
pub type RethResult<T> = Result<T, RethError>;

/// Core error variants possible when interacting with the blockchain.
///
/// This enum encapsulates various error types that can occur during blockchain interactions.
///
/// It allows for structured error handling based on the nature of the encountered issue.
#[derive(Debug, thiserror::Error)]
pub enum RethError {
    /// Error encountered during block execution.
    #[error(transparent)]
    Execution(#[from] BlockExecutionError),

    /// Consensus-related errors.
    #[error(transparent)]
    Consensus(#[from] ConsensusError),

    /// Database-related errors.
    #[error(transparent)]
    Database(#[from] DatabaseError),

    /// Errors originating from providers.
    #[error(transparent)]
    Provider(#[from] ProviderError),

    /// Canonical errors encountered.
    #[error(transparent)]
    Canonical(#[from] CanonicalError),

    /// Any other error.
    #[error(transparent)]
    Other(Box<dyn core::error::Error + Send + Sync>),
}

impl RethError {
    /// Create a new `RethError` from a given error.
    pub fn other<E>(error: E) -> Self
    where
        E: core::error::Error + Send + Sync + 'static,
    {
        Self::Other(Box::new(error))
    }

    /// Create a new `RethError` from a given message.
    pub fn msg(msg: impl Display) -> Self {
        Self::Other(msg.to_string().into())
    }
}

impl From<BlockchainTreeError> for RethError {
    fn from(error: BlockchainTreeError) -> Self {
        Self::Canonical(CanonicalError::BlockchainTree(error))
    }
}

impl From<FsPathError> for RethError {
    fn from(err: FsPathError) -> Self {
        Self::other(err)
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

    static_assert_size!(RethError, 64);
    static_assert_size!(BlockExecutionError, 56);
    static_assert_size!(ConsensusError, 48);
    static_assert_size!(DatabaseError, 32);
    static_assert_size!(ProviderError, 48);
    static_assert_size!(CanonicalError, 56);
}

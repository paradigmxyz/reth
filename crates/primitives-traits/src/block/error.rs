//! Error types for the `block` module.

use crate::transaction::signed::RecoveryError;

/// Type alias for [`BlockRecoveryError`] with a [`SealedBlock`](crate::SealedBlock) value.
///
/// This error type is specifically used when recovering a sealed block fails.
/// It contains the original sealed block that could not be recovered, allowing
/// callers to inspect the problematic block or attempt recovery with different
/// parameters.
///
/// # Example
///
/// ```rust
/// use reth_primitives_traits::block::error::SealedBlockRecoveryError;
/// use reth_primitives_traits::{SealedBlock, Block};
///
/// // Simulate a block recovery operation that fails
/// let sealed_block = SealedBlock::new_unchecked(Block::default(), Default::default());
/// let block_recovery_result: Result<_, SealedBlockRecoveryError<_>> =
///     Err(SealedBlockRecoveryError::new(sealed_block));
///
/// // When block recovery fails, you get the error with the original block
/// let error = block_recovery_result.unwrap_err();
/// let failed_block = error.into_inner();
/// // Now you can inspect the failed block or try recovery again
/// ```
pub type SealedBlockRecoveryError<B> = BlockRecoveryError<crate::SealedBlock<B>>;

/// Error when recovering a block from [`SealedBlock`](crate::SealedBlock) to
/// [`RecoveredBlock`](crate::RecoveredBlock).
///
/// This error is returned when the block recovery fails and contains the erroneous block, because
/// recovering a block takes ownership of the block.
#[derive(Debug, Clone, thiserror::Error)]
#[error("Failed to recover the block")]
pub struct BlockRecoveryError<T>(pub T);

impl<T> BlockRecoveryError<T> {
    /// Create a new error.
    pub const fn new(inner: T) -> Self {
        Self(inner)
    }

    /// Unwrap the error and return the original value.
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> From<BlockRecoveryError<T>> for RecoveryError
where
    T: core::fmt::Debug + Send + Sync + 'static,
{
    fn from(err: BlockRecoveryError<T>) -> Self {
        Self::from_source(err)
    }
}

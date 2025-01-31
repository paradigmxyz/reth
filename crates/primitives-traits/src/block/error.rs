//! Error types for the `block` module.

use crate::transaction::signed::RecoveryError;

/// Type alias for [`BlockRecoveryError`] with a [`SealedBlock`](crate::SealedBlock) value.
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

impl<T> From<BlockRecoveryError<T>> for RecoveryError {
    fn from(_: BlockRecoveryError<T>) -> Self {
        Self
    }
}

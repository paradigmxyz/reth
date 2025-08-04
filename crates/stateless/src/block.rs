use reth_primitives_traits::{block::error::BlockRecoveryError, Block, RecoveredBlock};

/// Abstraction of a [`Block`] which can be converted into a [`RecoveredBlock`], i.e. a block with
/// senders recovered from the block's transactions.
pub trait RecoverableBlock<B: Block> {
    /// Transform into a [`RecoveredBlock`] by recovering senders in the contained transactions.
    fn try_into_recovered(self) -> Result<RecoveredBlock<B>, BlockRecoveryError<B>>;
}

impl<B: Block> RecoverableBlock<B> for RecoveredBlock<B> {
    #[inline]
    fn try_into_recovered(self) -> Result<Self, BlockRecoveryError<B>> {
        Ok(self)
    }
}

impl<B: Block> RecoverableBlock<B> for B {
    #[inline]
    fn try_into_recovered(self) -> Result<RecoveredBlock<B>, BlockRecoveryError<B>> {
        self.try_into_recovered()
    }
}

use crate::{RecoveredBlock, SealedBlock};
use alloc::vec::Vec;
use reth_primitives_traits::{Block, BlockBody, SignedTransaction};
use revm_primitives::{Address, B256};

/// Extension trait for [`reth_primitives_traits::Block`] implementations
/// allowing for conversions into common block parts containers such as [`SealedBlock`],
/// [`RecoveredBlock`], etc.
pub trait BlockExt: Block {
    /// Calculate the header hash and seal the block so that it can't be changed.
    fn seal_slow(self) -> SealedBlock<Self> {
        SealedBlock::seal(self)
    }

    /// Seal the block with a known hash.
    ///
    /// WARNING: This method does not perform validation whether the hash is correct.
    fn seal(self, hash: B256) -> SealedBlock<Self> {
        SealedBlock::new(self, hash)
    }

    /// Expensive operation that recovers transaction signer.
    fn senders(&self) -> Option<Vec<Address>>
    where
        <Self::Body as BlockBody>::Transaction: SignedTransaction,
    {
        self.body().recover_signers()
    }

    /// Transform into a [`RecoveredBlock`].
    ///
    /// # Panics
    ///
    /// If the number of senders does not match the number of transactions in the block
    /// and the signer recovery for one of the transactions fails.
    ///
    /// Note: this is expected to be called with blocks read from disk.
    #[track_caller]
    fn with_senders_unchecked(self, senders: Vec<Address>) -> RecoveredBlock<Self>
    where
        <Self::Body as BlockBody>::Transaction: SignedTransaction,
    {
        self.try_with_senders_unchecked(senders).expect("stored block is valid")
    }

    /// Transform into a [`RecoveredBlock`] using the given senders.
    ///
    /// If the number of senders does not match the number of transactions in the block, this falls
    /// back to manually recovery, but _without ensuring that the signature has a low `s` value_.
    ///
    /// Returns an error if a signature is invalid.
    #[track_caller]
    fn try_with_senders_unchecked(self, senders: Vec<Address>) -> Result<RecoveredBlock<Self>, Self>
    where
        <Self::Body as BlockBody>::Transaction: SignedTransaction,
    {
        let senders = if self.body().transactions().len() == senders.len() {
            senders
        } else {
            let Some(senders) = self.body().recover_signers_unchecked() else { return Err(self) };
            senders
        };

        Ok(RecoveredBlock::new_unhashed(self, senders))
    }

    /// **Expensive**. Transform into a [`RecoveredBlock`] by recovering senders in the contained
    /// transactions.
    ///
    /// Returns `None` if a transaction is invalid.
    fn with_recovered_senders(self) -> Option<RecoveredBlock<Self>>
    where
        <Self::Body as BlockBody>::Transaction: SignedTransaction,
    {
        let senders = self.senders()?;
        Some(RecoveredBlock::new_unhashed(self, senders))
    }
}

impl<T: Block> BlockExt for T {}

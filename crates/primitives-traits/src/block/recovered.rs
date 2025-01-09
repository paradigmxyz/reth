//! Recovered Block variant.

use crate::{
    block::SealedBlock2,
    sync::OnceLock,
    transaction::signed::{RecoveryError, SignedTransactionIntoRecoveredExt},
    Block, BlockBody, InMemorySize,
};
use alloy_consensus::transaction::Recovered;
use alloy_primitives::{Address, BlockHash, Sealable};
use derive_more::Deref;

/// A block with senders recovered from transactions.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deref)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RecoveredBlock<B> {
    /// Block hash
    #[cfg_attr(feature = "serde", serde(skip))]
    hash: OnceLock<BlockHash>,
    /// Block
    #[deref]
    block: B,
    /// List of senders that match the transactions in the block
    senders: Vec<Address>,
}

impl<B> RecoveredBlock<B> {
    /// Creates a new recovered block instance with the given senders as provided and the block
    /// hash.
    pub fn new(block: B, senders: Vec<Address>, hash: BlockHash) -> Self {
        Self { hash: hash.into(), block, senders }
    }
    /// Creates a new recovered block instance with the given senders as provided
    pub fn new_unhashed(block: B, senders: Vec<Address>) -> Self {
        Self { hash: Default::default(), block, senders }
    }
}

impl<B: Block> RecoveredBlock<B> {
    /// A safer variant of [`Self::new_unhashed`] that checks if the number of senders is equal to
    /// the number of transactions in the block and recovers the senders from the transactions, if
    /// not using [`SignedTransaction::recover_signer`](crate::transaction::signed::SignedTransaction)
    /// to recover the senders.
    pub fn try_new(
        block: B,
        senders: Vec<Address>,
        hash: BlockHash,
    ) -> Result<Self, RecoveryError> {
        let senders = if block.body().transaction_count() == senders.len() {
            senders
        } else {
            block.body().try_recover_signers()?
        };
        Ok(Self::new(block, senders, hash))
    }

    /// A safer variant of [`Self::new`] that checks if the number of senders is equal to
    /// the number of transactions in the block and recovers the senders from the transactions, if
    /// not using [`SignedTransaction::recover_signer_unchecked`](crate::transaction::signed::SignedTransaction)
    /// to recover the senders.
    pub fn try_new_unchecked(
        block: B,
        senders: Vec<Address>,
        hash: BlockHash,
    ) -> Result<Self, RecoveryError> {
        let senders = if block.body().transaction_count() == senders.len() {
            senders
        } else {
            block.body().try_recover_signers_unchecked()?
        };
        Ok(Self::new(block, senders, hash))
    }

    /// A safer variant of [`Self::new_unhashed`] that checks if the number of senders is equal to
    /// the number of transactions in the block and recovers the senders from the transactions, if
    /// not using [`SignedTransaction::recover_signer`](crate::transaction::signed::SignedTransaction)
    /// to recover the senders.
    pub fn try_new_unhashed(block: B, senders: Vec<Address>) -> Result<Self, RecoveryError> {
        let senders = if block.body().transaction_count() == senders.len() {
            senders
        } else {
            block.body().try_recover_signers()?
        };
        Ok(Self::new_unhashed(block, senders))
    }

    /// A safer variant of [`Self::new_unhashed`] that checks if the number of senders is equal to
    /// the number of transactions in the block and recovers the senders from the transactions, if
    /// not using [`SignedTransaction::recover_signer_unchecked`](crate::transaction::signed::SignedTransaction)
    /// to recover the senders.
    pub fn try_new_unhashed_unchecked(
        block: B,
        senders: Vec<Address>,
    ) -> Result<Self, RecoveryError> {
        let senders = if block.body().transaction_count() == senders.len() {
            senders
        } else {
            block.body().try_recover_signers_unchecked()?
        };
        Ok(Self::new_unhashed(block, senders))
    }

    /// Recovers the senders from the transactions in the block using
    /// [`SignedTransaction::recover_signer`].
    ///
    /// Returns an error if any of the transactions fail to recover the sender.
    pub fn try_recover(block: B) -> Result<Self, RecoveryError> {
        let senders = block.body().try_recover_signers()?;
        Ok(Self::new_unhashed(block, senders))
    }

    /// Recovers the senders from the transactions in the block using
    /// [`SignedTransaction::recover_signer_unchecked`].
    ///
    /// Returns an error if any of the transactions fail to recover the sender.
    pub fn try_recover_unchecked(block: B) -> Result<Self, RecoveryError> {
        let senders = block.body().try_recover_signers_unchecked()?;
        Ok(Self::new_unhashed(block, senders))
    }

    /// Recovers the senders from the transactions in the block using
    /// [`SignedTransaction::recover_signer`].
    ///
    /// Returns an error if any of the transactions fail to recover the sender.
    pub fn try_recover_sealed(block: SealedBlock2<B>) -> Result<Self, RecoveryError> {
        let senders = block.body().try_recover_signers()?;
        let (block, hash) = block.split();
        Ok(Self::new(block, senders, hash))
    }

    /// Recovers the senders from the transactions in the sealed block using
    /// [`SignedTransaction::recover_signer_unchecked`].
    ///
    /// Returns an error if any of the transactions fail to recover the sender.
    pub fn try_recover_sealed_unchecked(block: SealedBlock2<B>) -> Result<Self, RecoveryError> {
        let senders = block.body().try_recover_signers_unchecked()?;
        let (block, hash) = block.split();
        Ok(Self::new(block, senders, hash))
    }

    /// Returns the block hash.
    pub fn hash_ref(&self) -> &BlockHash {
        self.hash.get_or_init(|| self.block.header().hash_slow())
    }

    /// Returns a copy of the block hash.
    pub fn hash(&self) -> BlockHash {
        *self.hash_ref()
    }

    /// Consumes the block and returns the [`SealedBlock2`] and drops the recovered senders.
    pub fn into_sealed_block(self) -> SealedBlock2<B> {
        let hash = self.hash();
        SealedBlock2::new(self.block, hash)
    }

    /// Consumes the type and returns its components.
    pub fn split_sealed(self) -> (SealedBlock2<B>, Vec<Address>) {
        let hash = self.hash();
        (SealedBlock2::new(self.block, hash), self.senders)
    }

    /// Consumes the type and returns its components.
    pub fn split(self) -> (B, Vec<Address>) {
        (self.block, self.senders)
    }

    /// Returns an iterator over all transactions and their sender.
    #[inline]
    pub fn transactions_with_sender(
        &self,
    ) -> impl Iterator<Item = (&Address, &<B::Body as BlockBody>::Transaction)> + '_ {
        self.senders.iter().zip(self.block.body().transactions())
    }

    /// Returns an iterator over all transactions in the block.
    #[inline]
    pub fn into_transactions_ecrecovered(
        self,
    ) -> impl Iterator<Item = Recovered<<B::Body as BlockBody>::Transaction>> {
        self.block
            .split()
            .1
            .into_transactions()
            .into_iter()
            .zip(self.senders)
            .map(|(tx, sender)| tx.with_signer(sender))
    }

    /// Consumes the block and returns the transactions of the block.
    #[inline]
    pub fn into_transactions(self) -> Vec<<B::Body as BlockBody>::Transaction> {
        self.block.split().1.into_transactions()
    }
}

impl<B: InMemorySize> InMemorySize for RecoveredBlock<B> {
    #[inline]
    fn size(&self) -> usize {
        self.block.size() +
            core::mem::size_of::<BlockHash>() +
            self.senders.len() * core::mem::size_of::<Address>()
    }
}

#[cfg(any(test, feature = "arbitrary"))]
impl<'a, B> arbitrary::Arbitrary<'a> for RecoveredBlock<B>
where
    B: Block + arbitrary::Arbitrary<'a>,
{
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        let block = B::arbitrary(u)?;
        Ok(Self::try_recover(block).unwrap())
    }
}

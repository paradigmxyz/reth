//! Recovered Block variant.

use crate::{
    block::SealedBlock2, sync::OnceLock, transaction::signed::RecoveryError, Block, BlockBody,
};
use alloy_primitives::{Address, BlockHash, Sealable};
use derive_more::Deref;

/// A block with senders recovered from transactions.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deref)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RecoveredBlock<B> {
    /// Block hash
    #[cfg_attr(feature = "serde", skip)]
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
    pub fn new_unchecked(block: B, senders: Vec<Address>, hash: BlockHash) -> Self {
        Self { hash: hash.into(), block, senders }
    }
    /// Creates a new recovered block instance with the given senders as provided
    pub fn new_unhashed_unchecked(block: B, senders: Vec<Address>) -> Self {
        Self { hash: Default::default(), block, senders }
    }
}

impl<B: Block> RecoveredBlock<B> {
    /// Returns the block hash.
    pub fn hash_ref(&self) -> &BlockHash {
        self.hash.get_or_init(|| self.block.header().hash_slow())
    }

    /// Recovers the senders from the transactions in the block using
    /// [`SignedTransaction::recover_signer`].
    ///
    /// Returns an error if any of the transactions fail to recover the sender.
    pub fn try_recover(block: B) -> Result<Self, RecoveryError> {
        let senders = block.body().try_recover_signers()?;
        Ok(Self::new_unhashed_unchecked(block, senders))
    }

    /// Recovers the senders from the transactions in the block using
    /// [`SignedTransaction::recover_signer_unchecked`].
    ///
    /// Returns an error if any of the transactions fail to recover the sender.
    pub fn try_recover_unchecked(block: B) -> Result<Self, RecoveryError> {
        let senders = block.body().try_recover_signers_unchecked()?;
        Ok(Self::new_unhashed_unchecked(block, senders))
    }

    /// Recovers the senders from the transactions in the block using
    /// [`SignedTransaction::recover_signer`].
    ///
    /// Returns an error if any of the transactions fail to recover the sender.
    pub fn try_recover_sealed(block: SealedBlock2<B>) -> Result<Self, RecoveryError> {
        let senders = block.body().try_recover_signers()?;
        let (block, hash) = block.split();
        Ok(Self::new_unchecked(block, senders, hash))
    }

    /// Recovers the senders from the transactions in the sealed block using
    /// [`SignedTransaction::recover_signer_unchecked`].
    ///
    /// Returns an error if any of the transactions fail to recover the sender.
    pub fn try_recover_sealed_unchecked(block: SealedBlock2<B>) -> Result<Self, RecoveryError> {
        let senders = block.body().try_recover_signers_unchecked()?;
        let (block, hash) = block.split();
        Ok(Self::new_unchecked(block, senders, hash))
    }
}

use crate::{
    transaction::{recover_signers, recover_signers_unchecked},
    BlockWithSenders, SealedBlock,
};
use alloc::vec::Vec;
use alloy_eips::{eip2718::Encodable2718, BlockNumHash};
use reth_primitives_traits::{Block, BlockBody, BlockHeader, SealedHeader, SignedTransaction};
use revm_primitives::{Address, B256};

/// Extension trait for [`reth_primitives_traits::Block`] implementations
/// allowing for conversions into common block parts containers such as [`SealedBlock`],
/// [`BlockWithSenders`], etc.
pub trait BlockExt: Block {
    /// Calculate the header hash and seal the block so that it can't be changed.
    fn seal_slow(self) -> SealedBlock<Self::Header, Self::Body> {
        let (header, body) = self.split();
        SealedBlock { header: SealedHeader::seal(header), body }
    }

    /// Seal the block with a known hash.
    ///
    /// WARNING: This method does not perform validation whether the hash is correct.
    fn seal(self, hash: B256) -> SealedBlock<Self::Header, Self::Body> {
        let (header, body) = self.split();
        SealedBlock { header: SealedHeader::new(header, hash), body }
    }

    /// Expensive operation that recovers transaction signer.
    fn senders(&self) -> Option<Vec<Address>>
    where
        <Self::Body as BlockBody>::Transaction: SignedTransaction,
    {
        self.body().recover_signers()
    }

    /// Transform into a [`BlockWithSenders`].
    ///
    /// # Panics
    ///
    /// If the number of senders does not match the number of transactions in the block
    /// and the signer recovery for one of the transactions fails.
    ///
    /// Note: this is expected to be called with blocks read from disk.
    #[track_caller]
    fn with_senders_unchecked(self, senders: Vec<Address>) -> BlockWithSenders<Self>
    where
        <Self::Body as BlockBody>::Transaction: SignedTransaction,
    {
        self.try_with_senders_unchecked(senders).expect("stored block is valid")
    }

    /// Transform into a [`BlockWithSenders`] using the given senders.
    ///
    /// If the number of senders does not match the number of transactions in the block, this falls
    /// back to manually recovery, but _without ensuring that the signature has a low `s` value_.
    /// See also [`recover_signers_unchecked`]
    ///
    /// Returns an error if a signature is invalid.
    #[track_caller]
    fn try_with_senders_unchecked(
        self,
        senders: Vec<Address>,
    ) -> Result<BlockWithSenders<Self>, Self>
    where
        <Self::Body as BlockBody>::Transaction: SignedTransaction,
    {
        let senders = if self.body().transactions().len() == senders.len() {
            senders
        } else {
            let Some(senders) = self.body().recover_signers_unchecked() else { return Err(self) };
            senders
        };

        Ok(BlockWithSenders::new_unchecked(self, senders))
    }

    /// **Expensive**. Transform into a [`BlockWithSenders`] by recovering senders in the contained
    /// transactions.
    ///
    /// Returns `None` if a transaction is invalid.
    fn with_recovered_senders(self) -> Option<BlockWithSenders<Self>>
    where
        <Self::Body as BlockBody>::Transaction: SignedTransaction,
    {
        let senders = self.senders()?;
        Some(BlockWithSenders::new_unchecked(self, senders))
    }
}

impl<T: Block> BlockExt for T {}

/// Extension trait for [`BlockBody`] adding helper methods operating with transactions.
pub trait BlockBodyTxExt: BlockBody {
    /// Calculate the transaction root for the block body.
    fn calculate_tx_root(&self) -> B256
    where
        Self::Transaction: Encodable2718,
    {
        crate::proofs::calculate_transaction_root(self.transactions())
    }

    /// Recover signer addresses for all transactions in the block body.
    fn recover_signers(&self) -> Option<Vec<Address>>
    where
        Self::Transaction: SignedTransaction,
    {
        recover_signers(self.transactions(), self.transactions().len())
    }

    /// Recover signer addresses for all transactions in the block body _without ensuring that the
    /// signature has a low `s` value_.
    ///
    /// Returns `None`, if some transaction's signature is invalid, see also
    /// [`recover_signers_unchecked`].
    fn recover_signers_unchecked(&self) -> Option<Vec<Address>>
    where
        Self::Transaction: SignedTransaction,
    {
        recover_signers_unchecked(self.transactions(), self.transactions().len())
    }
}

impl<T: BlockBody> BlockBodyTxExt for T {}

/// Extension trait for [`BlockHeader`] adding useful helper methods.
pub trait HeaderExt: BlockHeader {
    /// TODO: remove once <https://github.com/alloy-rs/alloy/pull/1687> is released
    ///
    /// Returns the parent block's number and hash
    ///
    /// Note: for the genesis block the parent number is 0 and the parent hash is the zero hash.
    fn parent_num_hash(&self) -> BlockNumHash {
        BlockNumHash::new(self.number().saturating_sub(1), self.parent_hash())
    }
}

impl<T: BlockHeader> HeaderExt for T {}

//! Block abstraction.

pub mod body;
pub mod header;

use alloc::{fmt, vec::Vec};

use alloy_eips::eip7685::Requests;
use alloy_primitives::{Address, B256};
use reth_codecs::Compact;

use crate::{BlockBody, BlockHeader, Body, FullBlockBody, FullBlockHeader, InMemorySize};

/// Helper trait that unifies all behaviour required by block to support full node operations.
pub trait FullBlock: Block<Header: FullBlockHeader, Body: FullBlockBody> + Compact {}

impl<T> FullBlock for T where T: Block<Header: FullBlockHeader, Body: FullBlockBody> + Compact {}

/// Abstraction of block data type.
// todo: make sealable super-trait, depends on <https://github.com/paradigmxyz/reth/issues/11449>
// todo: make with senders extension trait, so block can be impl by block type already containing
// senders
pub trait Block:
    Send
    + Sync
    + Unpin
    + Clone
    + Default
    + fmt::Debug
    + PartialEq
    + Eq
    + serde::Serialize
    + for<'a> serde::Deserialize<'a>
    + From<(Self::Header, Self::Body)>
    + Into<(Self::Header, Self::Body)>
    + alloy_rlp::Encodable
    + alloy_rlp::Decodable
    + alloy_consensus::BlockHeader
    + Body<
        Ommer = Self::Header,
        SignedTransaction = <Self::Body as Body>::SignedTransaction,
        Withdrawals = <Self::Body as Body>::Withdrawals,
    > + InMemorySize
{
    /// Header part of the block.
    type Header: BlockHeader;

    /// The block's body contains the transactions in the block.
    type Body: BlockBody<Ommer = Self::Header>;

    /// A block and block hash.
    type SealedBlock<H, B>;

    /// A block and addresses of senders of transactions in it.
    type BlockWithSenders<T>;

    /// Returns reference to [`BlockHeader`] type.
    fn header(&self) -> &Self::Header;

    /// Returns reference to [`BlockBody`] type.
    fn body(&self) -> &Self::Body;

    /// Calculate the header hash and seal the block so that it can't be changed.
    // todo: can be default impl with <https://github.com/paradigmxyz/reth/issues/11449>
    fn seal_slow(self) -> Self::SealedBlock<Self::Header, Self::Body>;

    /// Seal the block with a known hash.
    ///
    /// WARNING: This method does not perform validation whether the hash is correct.
    // todo: can be default impl with <https://github.com/paradigmxyz/reth/issues/11449>
    fn seal(self, hash: B256) -> Self::SealedBlock<Self::Header, Self::Body>;

    /// Expensive operation that recovers transaction signer. See
    /// `SealedBlockWithSenders`.
    fn senders(&self) -> Option<Vec<Address>> {
        self.body().recover_signers()
    }

    /// Transform into a `BlockWithSenders`.
    ///
    /// # Panics
    ///
    /// If the number of senders does not match the number of transactions in the block
    /// and the signer recovery for one of the transactions fails.
    ///
    /// Note: this is expected to be called with blocks read from disk.
    #[track_caller]
    fn with_senders_unchecked(self, senders: Vec<Address>) -> Self::BlockWithSenders<Self> {
        self.try_with_senders_unchecked(senders).expect("stored block is valid")
    }

    /// Transform into a `BlockWithSenders` using the given senders.
    ///
    /// If the number of senders does not match the number of transactions in the block, this falls
    /// back to manually recovery, but _without ensuring that the signature has a low `s` value_.
    /// See also `SignedTransaction::recover_signer_unchecked`.
    ///
    /// Returns an error if a signature is invalid.
    // todo: can be default impl with <https://github.com/paradigmxyz/reth/issues/11449>
    #[track_caller]
    fn try_with_senders_unchecked(
        self,
        senders: Vec<Address>,
    ) -> Result<Self::BlockWithSenders<Self>, Self>;

    /// **Expensive**. Transform into a `BlockWithSenders` by recovering senders in the contained
    /// transactions.
    ///
    /// Returns `None` if a transaction is invalid.
    // todo: can be default impl with <https://github.com/paradigmxyz/reth/issues/11449>
    fn with_recovered_senders(self) -> Option<Self::BlockWithSenders<Self>>;
}

impl<T: Block> Body for T {
    type SignedTransaction = <T::Body as Body>::SignedTransaction;
    type Ommer = T::Header;
    type Withdrawals = <T::Body as Body>::Withdrawals;

    fn transactions(&self) -> &[Self::SignedTransaction] {
        self.body().transactions()
    }

    fn withdrawals(&self) -> Option<&Self::Withdrawals> {
        self.body().withdrawals()
    }

    fn ommers(&self) -> &[Self::Ommer] {
        self.body().ommers()
    }

    fn requests(&self) -> Option<&Requests> {
        self.body().requests()
    }

    fn calculate_tx_root(&self) -> B256 {
        self.body().calculate_tx_root()
    }

    fn calculate_ommers_root(&self) -> B256 {
        self.body().calculate_ommers_root()
    }

    fn calculate_withdrawals_root(&self) -> Option<B256> {
        self.body().calculate_withdrawals_root()
    }

    fn recover_signers(&self) -> Option<Vec<Address>> {
        self.body().recover_signers()
    }

    fn blob_versioned_hashes(&self) -> &[B256] {
        self.body().blob_versioned_hashes()
    }
}

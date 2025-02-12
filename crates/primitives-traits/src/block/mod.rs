//! Block abstraction.

pub(crate) mod sealed;
pub use sealed::SealedBlock;

pub(crate) mod recovered;
pub use recovered::RecoveredBlock;

pub mod body;
pub mod error;
pub mod header;

use alloc::{fmt, vec::Vec};
use alloy_primitives::{Address, B256};
use alloy_rlp::{Decodable, Encodable};

use crate::{
    block::error::BlockRecoveryError, transaction::signed::RecoveryError, BlockBody, BlockHeader,
    FullBlockBody, FullBlockHeader, InMemorySize, MaybeSerde, SealedHeader, SignedTransaction,
};

/// Bincode-compatible header type serde implementations.
#[cfg(feature = "serde-bincode-compat")]
pub mod serde_bincode_compat {
    pub use super::{
        recovered::serde_bincode_compat::RecoveredBlock, sealed::serde_bincode_compat::SealedBlock,
    };
}

/// Helper trait that unifies all behaviour required by block to support full node operations.
pub trait FullBlock:
    Block<Header: FullBlockHeader, Body: FullBlockBody> + alloy_rlp::Encodable + alloy_rlp::Decodable
{
}

impl<T> FullBlock for T where
    T: Block<Header: FullBlockHeader, Body: FullBlockBody>
        + alloy_rlp::Encodable
        + alloy_rlp::Decodable
{
}

/// Helper trait to access [`BlockBody::Transaction`] given a [`Block`].
pub type BlockTx<B> = <<B as Block>::Body as BlockBody>::Transaction;

/// Abstraction of block data type.
///
/// This type defines the structure of a block in the blockchain.
/// A [`Block`] is composed of a header and a body.
/// It is expected that a block can always be completely reconstructed from its header and body.
pub trait Block:
    Send
    + Sync
    + Unpin
    + Clone
    + Default
    + fmt::Debug
    + PartialEq
    + Eq
    + InMemorySize
    + MaybeSerde
    + Encodable
    + Decodable
{
    /// Header part of the block.
    type Header: BlockHeader;

    /// The block's body contains the transactions in the block and additional data, e.g.
    /// withdrawals in ethereum.
    type Body: BlockBody<OmmerHeader = Self::Header>;

    /// Create new block instance.
    fn new(header: Self::Header, body: Self::Body) -> Self;

    /// Create new a sealed block instance from a sealed header and the block body.
    fn new_sealed(header: SealedHeader<Self::Header>, body: Self::Body) -> SealedBlock<Self> {
        SealedBlock::from_sealed_parts(header, body)
    }

    /// Seal the block with a known hash.
    ///
    /// WARNING: This method does not perform validation whether the hash is correct.
    fn seal_unchecked(self, hash: B256) -> SealedBlock<Self> {
        SealedBlock::new_unchecked(self, hash)
    }

    /// Calculate the header hash and seal the block so that it can't be changed.
    fn seal_slow(self) -> SealedBlock<Self> {
        SealedBlock::seal_slow(self)
    }

    /// Returns reference to block header.
    fn header(&self) -> &Self::Header;

    /// Returns reference to block body.
    fn body(&self) -> &Self::Body;

    /// Splits the block into its header and body.
    fn split(self) -> (Self::Header, Self::Body);

    /// Returns a tuple of references to the block's header and body.
    fn split_ref(&self) -> (&Self::Header, &Self::Body) {
        (self.header(), self.body())
    }

    /// Consumes the block and returns the header.
    fn into_header(self) -> Self::Header {
        self.split().0
    }

    /// Consumes the block and returns the body.
    fn into_body(self) -> Self::Body {
        self.split().1
    }

    /// Returns the rlp length of the block with the given header and body.
    fn rlp_length(header: &Self::Header, body: &Self::Body) -> usize;

    /// Expensive operation that recovers transaction signer.
    fn recover_signers(&self) -> Result<Vec<Address>, RecoveryError>
    where
        <Self::Body as BlockBody>::Transaction: SignedTransaction,
    {
        self.body().recover_signers()
    }

    /// Transform the block into a [`RecoveredBlock`] using the given senders.
    ///
    /// If the number of senders does not match the number of transactions in the block, this falls
    /// back to manually recovery, but _without ensuring that the signature has a low `s` value_.
    ///
    /// Returns the block as error if a signature is invalid.
    fn try_into_recovered_unchecked(
        self,
        senders: Vec<Address>,
    ) -> Result<RecoveredBlock<Self>, BlockRecoveryError<Self>>
    where
        <Self::Body as BlockBody>::Transaction: SignedTransaction,
    {
        let senders = if self.body().transactions().len() == senders.len() {
            senders
        } else {
            // Fall back to recovery if lengths don't match
            let Ok(senders) = self.body().recover_signers_unchecked() else {
                return Err(BlockRecoveryError::new(self))
            };
            senders
        };
        Ok(RecoveredBlock::new_unhashed(self, senders))
    }

    /// Transform the block into a [`RecoveredBlock`] using the given signers.
    ///
    /// Note: This method assumes the signers are correct and does not validate them.
    fn into_recovered_with_signers(self, signers: Vec<Address>) -> RecoveredBlock<Self>
    where
        <Self::Body as BlockBody>::Transaction: SignedTransaction,
    {
        RecoveredBlock::new_unhashed(self, signers)
    }

    /// **Expensive**. Transform into a [`RecoveredBlock`] by recovering senders in the contained
    /// transactions.
    ///
    /// Returns the block as error if a signature is invalid.
    fn try_into_recovered(self) -> Result<RecoveredBlock<Self>, BlockRecoveryError<Self>>
    where
        <Self::Body as BlockBody>::Transaction: SignedTransaction,
    {
        let Ok(signers) = self.body().recover_signers() else {
            return Err(BlockRecoveryError::new(self))
        };
        Ok(RecoveredBlock::new_unhashed(self, signers))
    }
}

impl<T, H> Block for alloy_consensus::Block<T, H>
where
    T: SignedTransaction,
    H: BlockHeader,
{
    type Header = H;
    type Body = alloy_consensus::BlockBody<T, H>;

    fn new(header: Self::Header, body: Self::Body) -> Self {
        Self { header, body }
    }

    fn header(&self) -> &Self::Header {
        &self.header
    }

    fn body(&self) -> &Self::Body {
        &self.body
    }

    fn split(self) -> (Self::Header, Self::Body) {
        (self.header, self.body)
    }

    fn rlp_length(header: &Self::Header, body: &Self::Body) -> usize {
        Self::rlp_length_for(header, body)
    }
}

/// An extension trait for [`Block`]s that allows for mutable access to the block's internals.
///
/// This allows for modifying the block's header and body for testing purposes.
#[cfg(any(test, feature = "test-utils"))]
pub trait TestBlock: Block<Header: crate::test_utils::TestHeader> {
    /// Returns mutable reference to block body.
    fn body_mut(&mut self) -> &mut Self::Body;

    /// Returns mutable reference to block header.
    fn header_mut(&mut self) -> &mut Self::Header;

    /// Updates the block header.
    fn set_header(&mut self, header: Self::Header);

    /// Updates the parent block hash.
    fn set_parent_hash(&mut self, hash: alloy_primitives::BlockHash) {
        crate::header::test_utils::TestHeader::set_parent_hash(self.header_mut(), hash);
    }

    /// Updates the block number.
    fn set_block_number(&mut self, number: alloy_primitives::BlockNumber) {
        crate::header::test_utils::TestHeader::set_block_number(self.header_mut(), number);
    }

    /// Updates the block state root.
    fn set_state_root(&mut self, state_root: alloy_primitives::B256) {
        crate::header::test_utils::TestHeader::set_state_root(self.header_mut(), state_root);
    }

    /// Updates the block difficulty.
    fn set_difficulty(&mut self, difficulty: alloy_primitives::U256) {
        crate::header::test_utils::TestHeader::set_difficulty(self.header_mut(), difficulty);
    }
}

#[cfg(any(test, feature = "test-utils"))]
impl<T, H> TestBlock for alloy_consensus::Block<T, H>
where
    T: SignedTransaction,
    H: crate::test_utils::TestHeader,
{
    fn body_mut(&mut self) -> &mut Self::Body {
        &mut self.body
    }

    fn header_mut(&mut self) -> &mut Self::Header {
        &mut self.header
    }

    fn set_header(&mut self, header: Self::Header) {
        self.header = header
    }
}

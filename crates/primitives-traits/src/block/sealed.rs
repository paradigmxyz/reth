//! Sealed block types

use crate::{Block, BlockBody, GotExpected, InMemorySize, SealedHeader};
use alloy_consensus::BlockHeader;
use alloy_primitives::{BlockHash, Sealable, B256};
use core::ops::Deref;

/// Sealed full block.
///
/// This type wraps the block type together with the block hash.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SealedBlock2<B> {
    /// Sealed Header hash.
    hash: BlockHash,
    /// Sealed full block with header and body.
    block: B,
}

impl<B> SealedBlock2<B> {
    /// Create a new sealed block instance using the block.
    #[inline]
    pub const fn new(block: B, hash: BlockHash) -> Self {
        Self { block, hash }
    }

    /// Header hash.
    #[inline]
    pub const fn hash(&self) -> B256 {
        self.hash
    }
}

impl<B> SealedBlock2<B>
where
    B: Block,
{
    /// Returns reference to block header.
    pub fn header(&self) -> &B::Header {
        self.block.header()
    }

    /// Returns reference to block body.
    pub fn body(&self) -> &B::Body {
        self.block.body()
    }

    /// Returns the block hash
    pub const fn block_hash(&self) -> &B256 {
        &self.hash
    }

    /// Returns the Sealed header.
    pub fn sealed_header(&self) -> SealedHeader<&B::Header> {
        SealedHeader::new(self.header(), self.hash)
    }

    /// Consumes the block and returns the sealed header.
    pub fn into_sealed_header(self) -> SealedHeader<B::Header> {
        SealedHeader::new(self.block.into_header(), self.hash)
    }

    /// Consumes the block and returns the header.
    pub fn into_header(self) -> B::Header {
        self.block.into_header()
    }

    /// Consumes the block and returns the body.
    pub fn into_body(self) -> B::Body {
        self.block.into_body()
    }

    /// Splits the block into body and header into separate components
    pub fn split_header_body(self) -> (B::Header, B::Body) {
        self.block.split()
    }

    /// Returns an iterator over all blob versioned hashes from the block body.
    #[inline]
    pub fn blob_versioned_hashes_iter(&self) -> impl Iterator<Item = &B256> + '_ {
        self.body().blob_versioned_hashes_iter()
    }

    /// Returns the number of transactions in the block.
    #[inline]
    pub fn transaction_count(&self) -> usize {
        self.body().transaction_count()
    }

    /// Ensures that the transaction root in the block header is valid.
    ///
    /// The transaction root is the Keccak 256-bit hash of the root node of the trie structure
    /// populated with each transaction in the transactions list portion of the block.
    ///
    /// # Returns
    ///
    /// Returns `Ok(())` if the calculated transaction root matches the one stored in the header,
    /// indicating that the transactions in the block are correctly represented in the trie.
    ///
    /// Returns `Err(error)` if the transaction root validation fails, providing a `GotExpected`
    /// error containing the calculated and expected roots.
    pub fn ensure_transaction_root_valid(&self) -> Result<(), GotExpected<B256>> {
        let calculated_root = self.body().calculate_tx_root();

        if self.header().transactions_root() != calculated_root {
            return Err(GotExpected {
                got: calculated_root,
                expected: self.header().transactions_root(),
            })
        }

        Ok(())
    }
}

impl<B: Block> SealedBlock2<B> {
    /// Hashes the header and creates a sealed block.
    pub fn seal(block: B) -> Self {
        let hash = block.header().hash_slow();
        Self::new(block, hash)
    }
}

impl<B> From<B> for SealedBlock2<B>
where
    B: Block,
{
    fn from(block: B) -> Self {
        Self::seal(block)
    }
}

impl<B> Default for SealedBlock2<B>
where
    B: Block + Default,
{
    fn default() -> Self {
        Self::seal(Default::default())
    }
}

#[cfg(any(test, feature = "arbitrary"))]
impl<'a, B> arbitrary::Arbitrary<'a> for SealedBlock2<B>
where
    B: Block + arbitrary::Arbitrary<'a>,
{
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        let block = B::arbitrary(u)?;
        Ok(SealedBlock2::seal(block))
    }
}

impl<B: InMemorySize> InMemorySize for SealedBlock2<B> {
    #[inline]
    fn size(&self) -> usize {
        self.block.size() + self.hash.size()
    }
}

impl<B: Block> Deref for SealedBlock2<B> {
    type Target = B::Header;

    fn deref(&self) -> &Self::Target {
        self.header()
    }
}

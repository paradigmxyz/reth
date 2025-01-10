//! Sealed block types

use crate::{
    block::RecoveredBlock, transaction::signed::RecoveryError, Block, BlockBody, GotExpected,
    InMemorySize, SealedHeader,
};
use alloc::vec::Vec;
use alloy_consensus::BlockHeader;
use alloy_eips::{eip1898::BlockWithParent, BlockNumHash};
use alloy_primitives::{Address, BlockHash, Sealable, B256};
use alloy_rlp::{Decodable, Encodable};
use bytes::BufMut;
use core::ops::Deref;

/// Sealed full block.
///
/// This type wraps the block type together with the block hash.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct SealedBlock<B: Block> {
    /// Sealed Header.
    header: SealedHeader<B::Header>,
    /// the block's body.
    body: B::Body,
}

impl<B: Block> SealedBlock<B> {
    /// Create a new sealed block instance using the block.
    #[inline]
    pub fn new(block: B, hash: BlockHash) -> Self {
        let (header, body) = block.split();
        Self { header: SealedHeader::new(header, hash), body }
    }

    /// Header hash.
    #[inline]
    pub fn hash(&self) -> B256 {
        self.header.hash()
    }

    /// Consumes the type and returns its components.
    #[doc(alias = "into_components")]
    pub fn split(self) -> (B, BlockHash) {
        let (header, hash) = self.header.split();
        (B::new(header, self.body), hash)
    }

    /// Consumes the type and returns the block.
    #[doc(alias = "unseal")]
    pub fn into_block(self) -> B {
        let header = self.header.unseal();
        B::new(header, self.body)
    }

    /// Clones the wrapped block.
    pub fn clone_block(&self) -> B
    where
        B: Clone,
    {
        B::new(self.header.clone_header(), self.body.clone())
    }
}

impl<B: Block> SealedBlock<B> {
    /// Creates the [`SealedBlock`] from the block's parts by hashing the header.
    pub fn seal_parts(header: B::Header, body: B::Body) -> Self {
        Self::seal(B::new(header, body))
    }

    /// Creates the [`SealedBlock`] from the block's parts.
    pub fn from_parts(header: B::Header, body: B::Body, hash: BlockHash) -> Self {
        Self::new(B::new(header, body), hash)
    }

    /// Creates the [`SealedBlock`] from the [`SealedHeader`] and the body.
    pub fn from_sealed_parts(header: SealedHeader<B::Header>, body: B::Body) -> Self {
        let (header, hash) = header.split();
        Self::from_parts(header, body, hash)
    }

    /// Converts this block into a [`RecoveredBlock`] with the given senders
    ///
    /// Note: This method assumes the senders are correct and does not validate them.
    pub fn with_senders(self, senders: Vec<Address>) -> RecoveredBlock<B> {
        RecoveredBlock::new_sealed(self, senders)
    }

    /// Converts this block into a [`RecoveredBlock`] with the given senders if the number of
    /// senders is equal to the number of transactions in the block and recovers the senders from
    /// the transactions, if
    /// not using [`SignedTransaction::recover_signer`](crate::transaction::signed::SignedTransaction)
    /// to recover the senders.
    ///
    /// Returns an error if any of the transactions fail to recover the sender.
    pub fn try_with_senders(
        self,
        senders: Vec<Address>,
    ) -> Result<RecoveredBlock<B>, RecoveryError> {
        RecoveredBlock::try_recover_sealed_with_senders(self, senders)
    }

    /// Converts this block into a [`RecoveredBlock`] with the given senders if the number of
    /// senders is equal to the number of transactions in the block and recovers the senders from
    /// the transactions, if
    /// not using [`SignedTransaction::recover_signer_unchecked`](crate::transaction::signed::SignedTransaction)
    /// to recover the senders.
    ///
    /// Returns an error if any of the transactions fail to recover the sender.
    pub fn try_with_senders_unchecked(
        self,
        senders: Vec<Address>,
    ) -> Result<RecoveredBlock<B>, RecoveryError> {
        RecoveredBlock::try_recover_sealed_with_senders_unchecked(self, senders)
    }

    /// Recovers the senders from the transactions in the block using
    /// [`SignedTransaction::recover_signer`](crate::transaction::signed::SignedTransaction).
    ///
    /// Returns an error if any of the transactions fail to recover the sender.
    pub fn try_recover(self) -> Result<RecoveredBlock<B>, RecoveryError> {
        RecoveredBlock::try_recover_sealed(self)
    }

    /// Recovers the senders from the transactions in the block using
    /// [`SignedTransaction::recover_signer_unchecked`](crate::transaction::signed::SignedTransaction).
    ///
    /// Returns an error if any of the transactions fail to recover the sender.
    pub fn try_recover_unchecked(self) -> Result<RecoveredBlock<B>, RecoveryError> {
        RecoveredBlock::try_recover_sealed_unchecked(self)
    }

    /// Returns reference to block header.
    pub const fn header(&self) -> &B::Header {
        self.header.header()
    }

    /// Returns reference to block body.
    pub const fn body(&self) -> &B::Body {
        &self.body
    }

    /// Recovers all senders from the transactions in the block.
    ///
    /// Returns `None` if any of the transactions fail to recover the sender.
    pub fn senders(&self) -> Option<Vec<Address>> {
        self.body().recover_signers()
    }

    /// Return the number hash tuple.
    pub fn num_hash(&self) -> BlockNumHash {
        BlockNumHash::new(self.number(), self.hash())
    }

    /// Return a [`BlockWithParent`] for this header.
    pub fn block_with_parent(&self) -> BlockWithParent {
        BlockWithParent { parent: self.parent_hash(), block: self.num_hash() }
    }

    /// Returns the Sealed header.
    pub fn sealed_header(&self) -> SealedHeader<&B::Header> {
        SealedHeader::new(self.header(), self.hash())
    }

    /// Clones the wrapped header and returns a [`SealedHeader`] sealed with the hash.
    pub fn clone_sealed_header(&self) -> SealedHeader<B::Header> {
        self.header.clone()
    }

    /// Consumes the block and returns the sealed header.
    pub fn into_sealed_header(self) -> SealedHeader<B::Header> {
        self.header
    }

    /// Consumes the block and returns the header.
    pub fn into_header(self) -> B::Header {
        self.header.unseal()
    }

    /// Consumes the block and returns the body.
    pub fn into_body(self) -> B::Body {
        self.body
    }

    /// Splits the block into body and header into separate components
    pub fn split_header_body(self) -> (B::Header, B::Body) {
        let header = self.header.unseal();
        (header, self.body)
    }

    /// Splits the block into body and header into separate components.
    pub fn split_sealed_header_body(self) -> (SealedHeader<B::Header>, B::Body) {
        (self.header, self.body)
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

impl<B: Block> SealedBlock<B> {
    /// Hashes the header and creates a sealed block.
    pub fn seal(block: B) -> Self {
        let hash = block.header().hash_slow();
        Self::new(block, hash)
    }
}

impl<B> From<B> for SealedBlock<B>
where
    B: Block,
{
    fn from(block: B) -> Self {
        Self::seal(block)
    }
}

impl<B> Default for SealedBlock<B>
where
    B: Block + Default,
{
    fn default() -> Self {
        Self::seal(Default::default())
    }
}

impl<B: Block> InMemorySize for SealedBlock<B> {
    #[inline]
    fn size(&self) -> usize {
        self.body.size() + self.header.size()
    }
}

impl<B: Block> Deref for SealedBlock<B> {
    type Target = B::Header;

    fn deref(&self) -> &Self::Target {
        self.header()
    }
}

impl<B: Block> Encodable for SealedBlock<B> {
    fn encode(&self, out: &mut dyn BufMut) {
        self.body.encode(out);
    }
}

impl<B: Block> Decodable for SealedBlock<B> {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let block = B::decode(buf)?;
        Ok(Self::seal(block))
    }
}

#[cfg(any(test, feature = "arbitrary"))]
impl<'a, B> arbitrary::Arbitrary<'a> for SealedBlock<B>
where
    B: Block + arbitrary::Arbitrary<'a>,
{
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        let block = B::arbitrary(u)?;
        Ok(Self::seal(block))
    }
}

#[cfg(any(test, feature = "test-utils"))]
impl<B: crate::test_utils::TestBlock> SealedBlock<B> {}

/// Bincode-compatible [`SealedBlock`] serde implementation.
#[cfg(feature = "serde-bincode-compat")]
pub(super) mod serde_bincode_compat {
    use crate::{
        serde_bincode_compat::{self, SerdeBincodeCompat},
        Block, SealedHeader,
    };
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use serde_with::{serde_as, DeserializeAs, SerializeAs};

    /// Bincode-compatible [`super::SealedBlock`] serde implementation.
    ///
    /// Intended to use with the [`serde_with::serde_as`] macro in the following way:
    /// ```rust
    /// use reth_primitives_traits::{block::SealedBlock, serde_bincode_compat, Block};
    /// use serde::{Deserialize, Serialize};
    /// use serde_with::serde_as;
    ///
    /// #[serde_as]
    /// #[derive(Serialize, Deserialize)]
    /// struct Data<T: Block + serde_bincode_compat::SerdeBincodeCompat> {
    ///     #[serde_as(as = "serde_bincode_compat::SealedBlock<'_, T>")]
    ///     block: SealedBlock<T>,
    /// }
    /// ```
    #[serde_as]
    #[derive(derive_more::Debug, Serialize, Deserialize)]
    #[debug(bound(T::BincodeRepr<'a>: core::fmt::Debug))]
    pub struct SealedBlock<'a, T: Block + SerdeBincodeCompat> {
        // header: SealedHeader<T::Header>,
        block: T::BincodeRepr<'a>,
    }

    impl<'a, T: Block + SerdeBincodeCompat> From<&'a super::SealedBlock<T>> for SealedBlock<'a, T> {
        fn from(value: &'a super::SealedBlock<T>) -> Self {
            todo!()
            // Self { header: value.hash, block: (&value.body).into() }
        }
    }

    impl<'a, T: Block + SerdeBincodeCompat> From<SealedBlock<'a, T>> for super::SealedBlock<T> {
        fn from(value: SealedBlock<'a, T>) -> Self {
            todo!()
            // Self { hash: value.header, body: value.block.into() }
        }
    }

    impl<T: Block + SerdeBincodeCompat> SerializeAs<super::SealedBlock<T>> for SealedBlock<'_, T> {
        fn serialize_as<S>(source: &super::SealedBlock<T>, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            todo!()
            // SealedBlock::from(source).serialize(serializer)
        }
    }

    impl<'de, T: Block + SerdeBincodeCompat> DeserializeAs<'de, super::SealedBlock<T>>
        for SealedBlock<'de, T>
    {
        fn deserialize_as<D>(deserializer: D) -> Result<super::SealedBlock<T>, D::Error>
        where
            D: Deserializer<'de>,
        {
            todo!()
            // SealedBlock::deserialize(deserializer).map(Into::into)
        }
    }

    impl<T: Block + SerdeBincodeCompat> SerdeBincodeCompat for super::SealedBlock<T> {
        type BincodeRepr<'a> = SealedBlock<'a, T>;
    }
}

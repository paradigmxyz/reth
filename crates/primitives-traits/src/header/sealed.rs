use crate::{sync::OnceLock, InMemorySize};
pub use alloy_consensus::Header;
use alloy_consensus::Sealed;
use alloy_eips::{eip1898::BlockWithParent, BlockNumHash};
use alloy_primitives::{keccak256, BlockHash, Sealable};
use alloy_rlp::{Decodable, Encodable};
use bytes::BufMut;
use core::mem;
use derive_more::{AsRef, Deref};

/// Seals the header with the block hash.
///
/// This type uses lazy sealing to avoid hashing the header until it is needed:
///
/// [`SealedHeader::new_unhashed`] creates a sealed header without hashing the header.
/// [`SealedHeader::new`] creates a sealed header with the corresponding block hash.
/// [`SealedHeader::hash`] computes the hash if it has not been computed yet.
#[derive(Debug, Clone, AsRef, Deref)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(any(test, feature = "reth-codec"), reth_codecs::add_arbitrary_tests(rlp))]
pub struct SealedHeader<H = Header> {
    /// Block hash
    #[cfg_attr(feature = "serde", serde(skip))]
    hash: OnceLock<BlockHash>,
    /// Locked Header fields.
    #[as_ref]
    #[deref]
    header: H,
}

impl<H> SealedHeader<H> {
    /// Creates the sealed header without hashing the header.
    #[inline]
    pub fn new_unhashed(header: H) -> Self {
        Self { header, hash: Default::default() }
    }

    /// Creates the sealed header with the corresponding block hash.
    #[inline]
    pub fn new(header: H, hash: BlockHash) -> Self {
        Self { header, hash: hash.into() }
    }

    /// Returns the sealed Header fields.
    #[inline]
    pub const fn header(&self) -> &H {
        &self.header
    }

    /// Clone the header.
    pub fn clone_header(&self) -> H
    where
        H: Clone,
    {
        self.header.clone()
    }

    /// Consumes the type and returns the wrapped header.
    pub fn into_header(self) -> H {
        self.header
    }

    /// Consumes the type and returns the wrapped header.
    pub fn unseal(self) -> H {
        self.header
    }

    /// Converts from &`SealedHeader<H>` to `SealedHeader<&H>`.
    pub fn sealed_ref(&self) -> SealedHeader<&H> {
        SealedHeader { hash: self.hash.clone(), header: &self.header }
    }
}

impl<H: Sealable> SealedHeader<H> {
    /// Hashes the header and creates a sealed header.
    pub fn seal_slow(header: H) -> Self {
        let hash = header.hash_slow();
        Self::new(header, hash)
    }

    /// Returns the block hash.
    ///
    /// Note: if the hash has not been computed yet, this will compute the hash:
    /// [`Sealable::hash_slow`].
    pub fn hash_ref(&self) -> &BlockHash {
        self.hash.get_or_init(|| self.header.hash_slow())
    }

    /// Returns a copy of the block hash.
    pub fn hash(&self) -> BlockHash {
        *self.hash_ref()
    }

    /// This is the inverse of [`Header::seal_slow`] which returns the raw header and hash.
    pub fn split(self) -> (H, BlockHash) {
        let hash = self.hash();
        (self.header, hash)
    }
}

impl<H: Sealable> SealedHeader<&H> {
    /// Maps a `SealedHeader<&H>` to a `SealedHeader<H>` by cloning the header.
    pub fn cloned(self) -> SealedHeader<H>
    where
        H: Clone,
    {
        let Self { hash, header } = self;
        SealedHeader { hash, header: header.clone() }
    }
}

impl<H: alloy_consensus::BlockHeader + Sealable> SealedHeader<H> {
    /// Return the number hash tuple.
    pub fn num_hash(&self) -> BlockNumHash {
        BlockNumHash::new(self.number(), self.hash())
    }

    /// Return a [`BlockWithParent`] for this header.
    pub fn block_with_parent(&self) -> BlockWithParent {
        BlockWithParent { parent: self.parent_hash(), block: self.num_hash() }
    }
}

impl<H: Sealable> Eq for SealedHeader<H> {}

impl<H: Sealable> PartialEq for SealedHeader<H> {
    fn eq(&self, other: &Self) -> bool {
        self.hash() == other.hash()
    }
}

impl<H: Sealable> core::hash::Hash for SealedHeader<H> {
    fn hash<Ha: core::hash::Hasher>(&self, state: &mut Ha) {
        self.hash().hash(state)
    }
}

impl<H: InMemorySize> InMemorySize for SealedHeader<H> {
    /// Calculates a heuristic for the in-memory size of the [`SealedHeader`].
    #[inline]
    fn size(&self) -> usize {
        self.header.size() + mem::size_of::<BlockHash>()
    }
}

impl<H: Sealable + Default> Default for SealedHeader<H> {
    fn default() -> Self {
        Self::seal_slow(H::default())
    }
}

impl Encodable for SealedHeader {
    fn encode(&self, out: &mut dyn BufMut) {
        self.header.encode(out);
    }
}

impl Decodable for SealedHeader {
    fn decode(buf: &mut &[u8]) -> alloy_rlp::Result<Self> {
        let b = &mut &**buf;
        let started_len = buf.len();

        // decode the header from temp buffer
        let header = Header::decode(b)?;

        // hash the consumed bytes, the rlp encoded header
        let consumed = started_len - b.len();
        let hash = keccak256(&buf[..consumed]);

        // update original buffer
        *buf = *b;

        Ok(Self::new(header, hash))
    }
}

impl<H: Sealable> From<SealedHeader<H>> for Sealed<H> {
    fn from(value: SealedHeader<H>) -> Self {
        let (header, hash) = value.split();
        Self::new_unchecked(header, hash)
    }
}

#[cfg(any(test, feature = "arbitrary"))]
impl<'a, H> arbitrary::Arbitrary<'a> for SealedHeader<H>
where
    H: for<'b> arbitrary::Arbitrary<'b> + Sealable,
{
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        let header = H::arbitrary(u)?;

        Ok(Self::seal_slow(header))
    }
}

#[cfg(any(test, feature = "test-utils"))]
impl<H: crate::test_utils::TestHeader> SealedHeader<H> {
    /// Updates the block header.
    pub fn set_header(&mut self, header: H) {
        self.header = header
    }

    /// Updates the block hash.
    pub fn set_hash(&mut self, hash: BlockHash) {
        self.hash = hash.into()
    }

    /// Returns a mutable reference to the header.
    pub fn header_mut(&mut self) -> &mut H {
        &mut self.header
    }

    /// Updates the parent block hash.
    pub fn set_parent_hash(&mut self, hash: BlockHash) {
        self.header.set_parent_hash(hash);
    }

    /// Updates the block number.
    pub fn set_block_number(&mut self, number: alloy_primitives::BlockNumber) {
        self.header.set_block_number(number);
    }

    /// Updates the block state root.
    pub fn set_state_root(&mut self, state_root: alloy_primitives::B256) {
        self.header.set_state_root(state_root);
    }

    /// Updates the block difficulty.
    pub fn set_difficulty(&mut self, difficulty: alloy_primitives::U256) {
        self.header.set_difficulty(difficulty);
    }
}

/// Bincode-compatible [`SealedHeader`] serde implementation.
#[cfg(feature = "serde-bincode-compat")]
pub(super) mod serde_bincode_compat {
    use crate::serde_bincode_compat::SerdeBincodeCompat;
    use alloy_primitives::{BlockHash, Sealable};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use serde_with::{DeserializeAs, SerializeAs};

    /// Bincode-compatible [`super::SealedHeader`] serde implementation.
    ///
    /// Intended to use with the [`serde_with::serde_as`] macro in the following way:
    /// ```rust
    /// use reth_primitives_traits::{serde_bincode_compat, SealedHeader};
    /// use serde::{Deserialize, Serialize};
    /// use serde_with::serde_as;
    ///
    /// #[serde_as]
    /// #[derive(Serialize, Deserialize)]
    /// struct Data {
    ///     #[serde_as(as = "serde_bincode_compat::SealedHeader")]
    ///     header: SealedHeader,
    /// }
    /// ```
    #[derive(derive_more::Debug, Serialize, Deserialize)]
    #[debug(bound(H::BincodeRepr<'a>: core::fmt::Debug))]
    pub struct SealedHeader<'a, H: Sealable + SerdeBincodeCompat = super::Header> {
        hash: BlockHash,
        header: H::BincodeRepr<'a>,
    }

    impl<'a, H: Sealable + SerdeBincodeCompat> From<&'a super::SealedHeader<H>>
        for SealedHeader<'a, H>
    {
        fn from(value: &'a super::SealedHeader<H>) -> Self {
            Self { hash: value.hash(), header: (&value.header).into() }
        }
    }

    impl<'a, H: Sealable + SerdeBincodeCompat> From<SealedHeader<'a, H>> for super::SealedHeader<H> {
        fn from(value: SealedHeader<'a, H>) -> Self {
            Self::new(value.header.into(), value.hash)
        }
    }

    impl SerializeAs<super::SealedHeader> for SealedHeader<'_> {
        fn serialize_as<S>(source: &super::SealedHeader, serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            SealedHeader::from(source).serialize(serializer)
        }
    }

    impl<'de> DeserializeAs<'de, super::SealedHeader> for SealedHeader<'de> {
        fn deserialize_as<D>(deserializer: D) -> Result<super::SealedHeader, D::Error>
        where
            D: Deserializer<'de>,
        {
            SealedHeader::deserialize(deserializer).map(Into::into)
        }
    }

    impl<H: Sealable + SerdeBincodeCompat> SerdeBincodeCompat for super::SealedHeader<H> {
        type BincodeRepr<'a> = SealedHeader<'a, H>;
    }

    #[cfg(test)]
    mod tests {
        use super::super::{serde_bincode_compat, SealedHeader};
        use arbitrary::Arbitrary;
        use rand::Rng;
        use serde::{Deserialize, Serialize};
        use serde_with::serde_as;

        #[test]
        fn test_sealed_header_bincode_roundtrip() {
            #[serde_as]
            #[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
            struct Data {
                #[serde_as(as = "serde_bincode_compat::SealedHeader")]
                transaction: SealedHeader,
            }

            let mut bytes = [0u8; 1024];
            rand::thread_rng().fill(&mut bytes[..]);
            let data = Data {
                transaction: SealedHeader::arbitrary(&mut arbitrary::Unstructured::new(&bytes))
                    .unwrap(),
            };

            let encoded = bincode::serialize(&data).unwrap();
            let decoded: Data = bincode::deserialize(&encoded).unwrap();
            assert_eq!(decoded, data);
        }
    }
}

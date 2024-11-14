use super::Header;
use crate::InMemorySize;
use alloy_consensus::Sealed;
use alloy_eips::BlockNumHash;
use alloy_primitives::{keccak256, BlockHash, Sealable};
use alloy_rlp::{Decodable, Encodable};
use bytes::BufMut;
use core::mem;
use derive_more::{AsRef, Deref};
use reth_codecs::add_arbitrary_tests;
use serde::{Deserialize, Serialize};

/// A [`Header`] that is sealed at a precalculated hash, use [`SealedHeader::unseal()`] if you want
/// to modify header.
#[derive(Debug, Clone, PartialEq, Eq, Hash, AsRef, Deref, Serialize, Deserialize)]
#[add_arbitrary_tests(rlp)]
pub struct SealedHeader<H = Header> {
    /// Locked Header hash.
    hash: BlockHash,
    /// Locked Header fields.
    #[as_ref]
    #[deref]
    header: H,
}

impl<H> SealedHeader<H> {
    /// Creates the sealed header with the corresponding block hash.
    #[inline]
    pub const fn new(header: H, hash: BlockHash) -> Self {
        Self { header, hash }
    }

    /// Returns the sealed Header fields.
    #[inline]
    pub const fn header(&self) -> &H {
        &self.header
    }

    /// Returns header/block hash.
    #[inline]
    pub const fn hash(&self) -> BlockHash {
        self.hash
    }

    /// Extract raw header that can be modified.
    pub fn unseal(self) -> H {
        self.header
    }

    /// This is the inverse of [`Header::seal_slow`] which returns the raw header and hash.
    pub fn split(self) -> (H, BlockHash) {
        (self.header, self.hash)
    }
}

impl<H: Sealable> SealedHeader<H> {
    /// Hashes the header and creates a sealed header.
    pub fn seal(header: H) -> Self {
        let hash = header.hash_slow();
        Self::new(header, hash)
    }
}

impl<H: alloy_consensus::BlockHeader> SealedHeader<H> {
    /// Return the number hash tuple.
    pub fn num_hash(&self) -> BlockNumHash {
        BlockNumHash::new(self.number(), self.hash)
    }
}

impl InMemorySize for SealedHeader {
    /// Calculates a heuristic for the in-memory size of the [`SealedHeader`].
    #[inline]
    fn size(&self) -> usize {
        self.header.size() + mem::size_of::<BlockHash>()
    }
}

impl<H: Sealable + Default> Default for SealedHeader<H> {
    fn default() -> Self {
        let sealed = H::default().seal_slow();
        let (header, hash) = sealed.into_parts();
        Self { header, hash }
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

        Ok(Self { header, hash })
    }
}

#[cfg(any(test, feature = "test-utils"))]
impl SealedHeader {
    /// Updates the block header.
    pub fn set_header(&mut self, header: Header) {
        self.header = header
    }

    /// Updates the block hash.
    pub fn set_hash(&mut self, hash: BlockHash) {
        self.hash = hash
    }

    /// Updates the parent block hash.
    pub fn set_parent_hash(&mut self, hash: BlockHash) {
        self.header.parent_hash = hash
    }

    /// Updates the block number.
    pub fn set_block_number(&mut self, number: alloy_primitives::BlockNumber) {
        self.header.number = number;
    }

    /// Updates the block state root.
    pub fn set_state_root(&mut self, state_root: alloy_primitives::B256) {
        self.header.state_root = state_root;
    }

    /// Updates the block difficulty.
    pub fn set_difficulty(&mut self, difficulty: alloy_primitives::U256) {
        self.header.difficulty = difficulty;
    }
}

impl<H> From<SealedHeader<H>> for Sealed<H> {
    fn from(value: SealedHeader<H>) -> Self {
        Self::new_unchecked(value.header, value.hash)
    }
}

#[cfg(any(test, feature = "arbitrary"))]
impl<'a> arbitrary::Arbitrary<'a> for SealedHeader {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        let header = Header::arbitrary(u)?;

        Ok(Self::seal(header))
    }
}

/// Bincode-compatible [`SealedHeader`] serde implementation.
#[cfg(feature = "serde-bincode-compat")]
pub(super) mod serde_bincode_compat {
    use alloy_consensus::serde_bincode_compat::Header;
    use alloy_primitives::BlockHash;
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
    #[derive(Debug, Serialize, Deserialize)]
    pub struct SealedHeader<'a> {
        hash: BlockHash,
        header: Header<'a>,
    }

    impl<'a> From<&'a super::SealedHeader> for SealedHeader<'a> {
        fn from(value: &'a super::SealedHeader) -> Self {
            Self { hash: value.hash, header: Header::from(&value.header) }
        }
    }

    impl<'a> From<SealedHeader<'a>> for super::SealedHeader {
        fn from(value: SealedHeader<'a>) -> Self {
            Self { hash: value.hash, header: value.header.into() }
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

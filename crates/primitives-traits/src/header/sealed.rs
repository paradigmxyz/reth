use super::Header;
use alloy_eips::BlockNumHash;
use alloy_primitives::{keccak256, BlockHash, Sealable};
#[cfg(any(test, feature = "test-utils"))]
use alloy_primitives::{BlockNumber, B256, U256};
use alloy_rlp::{Decodable, Encodable};
use bytes::BufMut;
use core::mem;
use derive_more::{AsRef, Deref};
use reth_codecs::{add_arbitrary_tests, Compact};
use serde::{Deserialize, Serialize};

/// A [`Header`] that is sealed at a precalculated hash, use [`SealedHeader::unseal()`] if you want
/// to modify header.
#[derive(Debug, Clone, PartialEq, Eq, Hash, AsRef, Deref, Serialize, Deserialize, Compact)]
#[add_arbitrary_tests(rlp, compact)]
pub struct SealedHeader {
    /// Locked Header hash.
    hash: BlockHash,
    /// Locked Header fields.
    #[as_ref]
    #[deref]
    header: Header,
}

impl SealedHeader {
    /// Creates the sealed header with the corresponding block hash.
    #[inline]
    pub const fn new(header: Header, hash: BlockHash) -> Self {
        Self { header, hash }
    }

    /// Returns the sealed Header fields.
    #[inline]
    pub const fn header(&self) -> &Header {
        &self.header
    }

    /// Returns header/block hash.
    #[inline]
    pub const fn hash(&self) -> BlockHash {
        self.hash
    }

    /// Extract raw header that can be modified.
    pub fn unseal(self) -> Header {
        self.header
    }

    /// This is the inverse of [`Header::seal_slow`] which returns the raw header and hash.
    pub fn split(self) -> (Header, BlockHash) {
        (self.header, self.hash)
    }

    /// Return the number hash tuple.
    pub fn num_hash(&self) -> BlockNumHash {
        BlockNumHash::new(self.number, self.hash)
    }

    /// Calculates a heuristic for the in-memory size of the [`SealedHeader`].
    #[inline]
    pub fn size(&self) -> usize {
        self.header.size() + mem::size_of::<BlockHash>()
    }
}

impl Default for SealedHeader {
    fn default() -> Self {
        let sealed = Header::default().seal_slow();
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
    pub fn set_block_number(&mut self, number: BlockNumber) {
        self.header.number = number;
    }

    /// Updates the block state root.
    pub fn set_state_root(&mut self, state_root: B256) {
        self.header.state_root = state_root;
    }

    /// Updates the block difficulty.
    pub fn set_difficulty(&mut self, difficulty: U256) {
        self.header.difficulty = difficulty;
    }
}

#[cfg(any(test, feature = "arbitrary"))]
impl<'a> arbitrary::Arbitrary<'a> for SealedHeader {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        let mut header = Header::arbitrary(u)?;
        header.gas_limit = (header.gas_limit as u64).into();
        header.gas_used = (header.gas_used as u64).into();
        header.base_fee_per_gas =
            header.base_fee_per_gas.map(|base_fee_per_gas| (base_fee_per_gas as u64).into());
        header.blob_gas_used =
            header.blob_gas_used.map(|blob_gas_used| (blob_gas_used as u64).into());
        header.excess_blob_gas =
            header.excess_blob_gas.map(|excess_blob_gas| (excess_blob_gas as u64).into());

        let sealed = header.seal_slow();
        let (header, seal) = sealed.into_parts();
        Ok(Self::new(header, seal))
    }
}

//! Block access list table models.

use crate::{
    table::{Compress, Decode, Decompress, Encode},
    DatabaseError,
};
use alloy_primitives::{keccak256, BlockNumber, Bytes, B256};
use bytes::BufMut;
use core::cmp::Ordering;
use reth_codecs::DecompressError;
use serde::{Deserialize, Serialize};

/// Encoded [`StoredBlockAccessListKey`] length.
const BLOCK_ACCESS_LIST_KEY_BYTES: usize = 8 + 32;

/// Hash prefix length in [`StoredBlockAccessList`] values.
const STORED_BLOCK_ACCESS_LIST_HASH_BYTES: usize = 32;

/// Block access list table key.
///
/// Encoded as block number followed by block hash so pruning can scan by block number.
#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct StoredBlockAccessListKey {
    block_number: BlockNumber,
    block_hash: B256,
}

impl StoredBlockAccessListKey {
    /// Creates a key from a block number/hash pair.
    pub const fn new(block_number: BlockNumber, block_hash: B256) -> Self {
        Self { block_number, block_hash }
    }

    /// Returns the smallest key for the given block number.
    pub const fn first_at_number(block_number: BlockNumber) -> Self {
        Self::new(block_number, B256::ZERO)
    }

    /// Returns the block number.
    pub const fn number(&self) -> BlockNumber {
        self.block_number
    }

    /// Returns the block hash.
    pub const fn hash(&self) -> B256 {
        self.block_hash
    }
}

impl Ord for StoredBlockAccessListKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.block_number
            .cmp(&other.block_number)
            .then_with(|| self.block_hash.as_slice().cmp(other.block_hash.as_slice()))
    }
}

impl PartialOrd for StoredBlockAccessListKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Encode for StoredBlockAccessListKey {
    type Encoded = [u8; BLOCK_ACCESS_LIST_KEY_BYTES];

    fn encode(self) -> Self::Encoded {
        let mut buf = [0u8; BLOCK_ACCESS_LIST_KEY_BYTES];
        buf[..8].copy_from_slice(&self.block_number.to_be_bytes());
        buf[8..].copy_from_slice(self.block_hash.as_slice());
        buf
    }
}

impl Decode for StoredBlockAccessListKey {
    fn decode(value: &[u8]) -> Result<Self, DatabaseError> {
        if value.len() != BLOCK_ACCESS_LIST_KEY_BYTES {
            return Err(DatabaseError::Decode)
        }

        let block_number =
            u64::from_be_bytes(value[..8].try_into().map_err(|_| DatabaseError::Decode)?);
        let block_hash = B256::decode(&value[8..])?;

        Ok(Self::new(block_number, block_hash))
    }
}

/// Stored block access list value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StoredBlockAccessList {
    /// Keccak hash carried by the source BAL, trusted without verification on decode.
    hash: B256,
    /// Raw BAL RLP bytes.
    raw: Bytes,
}

impl StoredBlockAccessList {
    /// Creates a stored BAL from raw bytes.
    pub fn new(raw: Bytes) -> Self {
        let hash = keccak256(&raw);
        Self::new_unchecked(hash, raw)
    }

    /// Creates a stored BAL from its hash and raw bytes without verifying that they match.
    pub const fn new_unchecked(hash: B256, raw: Bytes) -> Self {
        Self { hash, raw }
    }

    /// Returns the stored hash without verifying it against the raw bytes.
    pub const fn hash(&self) -> B256 {
        self.hash
    }

    /// Consumes the stored BAL and returns its raw bytes.
    pub fn into_raw(self) -> Bytes {
        self.raw
    }
}

impl Compress for StoredBlockAccessList {
    type Compressed = Vec<u8>;

    fn compress(self) -> Self::Compressed {
        let mut out = Vec::with_capacity(STORED_BLOCK_ACCESS_LIST_HASH_BYTES + self.raw.len());
        out.extend_from_slice(self.hash.as_slice());
        out.extend_from_slice(&self.raw);
        out
    }

    fn compress_to_buf<B: BufMut + AsMut<[u8]>>(&self, buf: &mut B) {
        buf.put_slice(self.hash.as_slice());
        buf.put_slice(&self.raw);
    }
}

impl Decompress for StoredBlockAccessList {
    fn decompress(value: &[u8]) -> Result<Self, DecompressError> {
        if value.len() < STORED_BLOCK_ACCESS_LIST_HASH_BYTES {
            return Err(DecompressError::new(StoredBlockAccessListDecodeError))
        }

        let hash = B256::from_slice(&value[..STORED_BLOCK_ACCESS_LIST_HASH_BYTES]);
        let raw = Bytes::copy_from_slice(&value[STORED_BLOCK_ACCESS_LIST_HASH_BYTES..]);

        Ok(Self::new_unchecked(hash, raw))
    }
}

/// Error returned when a stored BAL value is too short to contain its hash prefix.
#[derive(Debug, derive_more::Display, derive_more::Error)]
#[display("stored block access list value is missing its hash prefix")]
struct StoredBlockAccessListDecodeError;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::{Compress, Decompress};

    #[test]
    fn key_encodes_number_first() {
        let low_hash = B256::with_last_byte(0xff);
        let high_hash = B256::ZERO;
        let low_number = StoredBlockAccessListKey::new(1, low_hash).encode();
        let high_number = StoredBlockAccessListKey::new(2, high_hash).encode();

        assert!(low_number < high_number);
    }

    #[test]
    fn key_roundtrip() {
        let key = StoredBlockAccessListKey::new(42, B256::with_last_byte(7));
        let encoded = key.encode();

        assert_eq!(StoredBlockAccessListKey::decode(&encoded).unwrap(), key);
    }

    #[test]
    fn stored_bal_roundtrip() {
        let raw = Bytes::from_static(&[0xc0]);
        let stored = StoredBlockAccessList::new(raw.clone());
        let encoded = stored.clone().compress();
        let decoded = StoredBlockAccessList::decompress(&encoded).unwrap();

        assert_eq!(decoded, stored);
        assert_eq!(decoded.hash(), keccak256(&raw));
        assert_eq!(decoded.into_raw(), raw);
    }

    #[test]
    fn stored_bal_unchecked_preserves_hash_and_raw_bytes() {
        let hash = B256::with_last_byte(1);
        let raw = Bytes::from_static(&[0xc0]);
        let stored = StoredBlockAccessList::new_unchecked(hash, raw.clone());
        let encoded = stored.compress();

        assert_eq!(&encoded[..STORED_BLOCK_ACCESS_LIST_HASH_BYTES], hash.as_slice());
        assert_eq!(&encoded[STORED_BLOCK_ACCESS_LIST_HASH_BYTES..], raw.as_ref());
    }
}

//! Block access list database models.

use crate::{
    table::{Compress, Decode, Decompress, Encode},
    DatabaseError,
};
use alloy_eip7928::bal::RawBal;
use alloy_eips::NumHash;
use alloy_primitives::{keccak256, BlockNumber, Bytes, B256};
use bytes::BufMut;
use core::cmp::Ordering;
use reth_codecs::DecompressError;
use serde::{Deserialize, Serialize};

/// Number of encoded bytes in a [`StoredBlockAccessListKey`].
const BLOCK_ACCESS_LIST_KEY_BYTES: usize = 8 + 32;

/// Number of hash bytes prefixed to a stored BAL value.
const STORED_BLOCK_ACCESS_LIST_HASH_BYTES: usize = 32;

/// A stored block access list key ordered by block number first, then block hash.
#[derive(Debug, Default, Copy, Clone, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(transparent)]
pub struct StoredBlockAccessListKey(NumHash);

impl StoredBlockAccessListKey {
    /// Creates a new stored block access list key.
    pub const fn new(num_hash: NumHash) -> Self {
        Self(num_hash)
    }

    /// Returns the smallest key for the given block number.
    pub const fn first_at_number(block_number: BlockNumber) -> Self {
        Self::new(NumHash::new(block_number, B256::ZERO))
    }

    /// Returns the block number.
    pub const fn number(&self) -> BlockNumber {
        self.0.number
    }

    /// Returns the block number/hash pair.
    pub const fn num_hash(&self) -> NumHash {
        self.0
    }
}

impl Ord for StoredBlockAccessListKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0
            .number
            .cmp(&other.0.number)
            .then_with(|| self.0.hash.as_slice().cmp(other.0.hash.as_slice()))
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
        buf[..8].copy_from_slice(&self.0.number.to_be_bytes());
        buf[8..].copy_from_slice(self.0.hash.as_slice());
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

        Ok(Self::new(NumHash::new(block_number, block_hash)))
    }
}

/// Raw block access list bytes with a stored hash for corruption checks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StoredBlockAccessList {
    /// Keccak hash of the raw BAL bytes.
    hash: B256,
    /// Raw RLP BAL.
    raw: RawBal,
}

impl StoredBlockAccessList {
    /// Creates a stored BAL from raw RLP bytes.
    pub fn new(raw: RawBal) -> Self {
        let hash = keccak256(raw.as_raw().as_ref());
        Self { hash, raw }
    }

    /// Creates a stored BAL from raw RLP bytes and an expected hash.
    pub const fn new_unchecked(raw: RawBal, hash: B256) -> Self {
        Self { hash, raw }
    }

    fn has_valid_hash(&self) -> bool {
        keccak256(self.raw.as_raw().as_ref()) == self.hash
    }

    /// Consumes this value and returns the raw BAL if the stored hash matches.
    pub fn into_verified_raw(self) -> Result<RawBal, StoredBlockAccessListHashError> {
        if self.has_valid_hash() {
            Ok(self.raw)
        } else {
            Err(StoredBlockAccessListHashError)
        }
    }
}

/// Error returned when stored BAL bytes do not match their stored hash.
#[derive(Debug, derive_more::Display, derive_more::Error)]
#[display("stored block access list hash mismatch")]
pub struct StoredBlockAccessListHashError;

impl Compress for StoredBlockAccessList {
    type Compressed = Vec<u8>;

    fn compress(self) -> Self::Compressed {
        let raw = self.raw.as_raw();
        let mut out = Vec::with_capacity(STORED_BLOCK_ACCESS_LIST_HASH_BYTES + raw.len());
        out.extend_from_slice(self.hash.as_slice());
        out.extend_from_slice(raw.as_ref());
        out
    }

    fn compress_to_buf<B: BufMut + AsMut<[u8]>>(&self, buf: &mut B) {
        buf.put_slice(self.hash.as_slice());
        buf.put_slice(self.raw.as_raw().as_ref());
    }
}

impl Decompress for StoredBlockAccessList {
    fn decompress(value: &[u8]) -> Result<Self, DecompressError> {
        if value.len() < STORED_BLOCK_ACCESS_LIST_HASH_BYTES {
            return Err(DecompressError::new(StoredBlockAccessListDecodeError))
        }

        let hash = B256::from_slice(&value[..STORED_BLOCK_ACCESS_LIST_HASH_BYTES]);
        let raw =
            RawBal::new(Bytes::copy_from_slice(&value[STORED_BLOCK_ACCESS_LIST_HASH_BYTES..]));

        Ok(Self { hash, raw })
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
        let low_number = StoredBlockAccessListKey::new(NumHash::new(1, low_hash)).encode();
        let high_number = StoredBlockAccessListKey::new(NumHash::new(2, high_hash)).encode();

        assert!(low_number < high_number);
    }

    #[test]
    fn key_roundtrip() {
        let key = StoredBlockAccessListKey::new(NumHash::new(42, B256::with_last_byte(7)));
        let encoded = key.encode();

        assert_eq!(StoredBlockAccessListKey::decode(&encoded).unwrap(), key);
    }

    #[test]
    fn stored_bal_roundtrip_and_hash_check() {
        let raw = RawBal::from(Bytes::from_static(&[0xc0]));
        let stored = StoredBlockAccessList::new(raw.clone());
        let encoded = stored.clone().compress();
        let decoded = StoredBlockAccessList::decompress(&encoded).unwrap();

        assert_eq!(decoded, stored);
        assert_eq!(decoded.into_verified_raw().unwrap().as_raw(), raw.as_raw());
    }

    #[test]
    fn stored_bal_rejects_hash_mismatch() {
        let stored = StoredBlockAccessList::new_unchecked(
            RawBal::from(Bytes::from_static(&[0xc0])),
            B256::ZERO,
        );

        assert!(stored.into_verified_raw().is_err());
    }
}

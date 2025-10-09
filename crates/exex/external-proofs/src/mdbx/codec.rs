//! Custom codecs for MDBX storage
//!
//! This module provides compression/decompression implementations for types that
//! don't have built-in support, particularly `Option<T>` for tracking deletions.

use bytes::BufMut;
use reth_db_api::table::{Compress, Decompress};
use serde::{Deserialize, Serialize};

/// Wrapper type for `Option<T>` that implements `Compress` and `Decompress`
///
/// Encoding:
/// - `None` => empty byte array (length 0)
/// - `Some(value)` => compressed bytes of value (length > 0)
///
/// This assumes the inner type `T` always compresses to non-empty bytes when it exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaybeDeleted<T>(pub Option<T>);

impl<T> From<Option<T>> for MaybeDeleted<T> {
    fn from(opt: Option<T>) -> Self {
        Self(opt)
    }
}

impl<T> From<MaybeDeleted<T>> for Option<T> {
    fn from(maybe: MaybeDeleted<T>) -> Self {
        maybe.0
    }
}

impl<T: Compress> Compress for MaybeDeleted<T> {
    type Compressed = Vec<u8>;

    fn compress_to_buf<B: BufMut + AsMut<[u8]>>(&self, buf: &mut B) {
        match &self.0 {
            None => {
                // Empty = deleted, write nothing
            }
            Some(value) => {
                // Compress the inner value to the buffer
                value.compress_to_buf(buf);
            }
        }
    }
}

impl<T: Decompress> Decompress for MaybeDeleted<T> {
    fn decompress(value: &[u8]) -> Result<Self, reth_db_api::DatabaseError> {
        if value.is_empty() {
            // Empty = deleted
            Ok(MaybeDeleted(None))
        } else {
            // Non-empty = present
            let inner = T::decompress(value)?;
            Ok(MaybeDeleted(Some(inner)))
        }
    }
}

/// Newtype wrapper for (u64, B256) to implement Compress/Decompress
///
/// Used for storing block metadata (number + hash).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockNumberHash(pub u64, pub alloy_primitives::B256);

impl From<(u64, alloy_primitives::B256)> for BlockNumberHash {
    fn from((number, hash): (u64, alloy_primitives::B256)) -> Self {
        Self(number, hash)
    }
}

impl From<BlockNumberHash> for (u64, alloy_primitives::B256) {
    fn from(bnh: BlockNumberHash) -> Self {
        (bnh.0, bnh.1)
    }
}

impl BlockNumberHash {
    /// Create a new block number and hash pair
    pub const fn new(block_number: u64, hash: alloy_primitives::B256) -> Self {
        Self(block_number, hash)
    }

    /// Destructure into components
    pub const fn into_components(self) -> (u64, alloy_primitives::B256) {
        (self.0, self.1)
    }
}

impl Compress for BlockNumberHash {
    type Compressed = Vec<u8>;

    fn compress_to_buf<B: BufMut + AsMut<[u8]>>(&self, buf: &mut B) {
        // Encode block number (8 bytes, big-endian) + hash (32 bytes) = 40 bytes total
        buf.put_u64(self.0);
        buf.put_slice(self.1.as_slice());
    }
}

impl Decompress for BlockNumberHash {
    fn decompress(value: &[u8]) -> Result<Self, reth_db_api::DatabaseError> {
        if value.len() != 40 {
            return Err(reth_db_api::DatabaseError::Decode);
        }

        let block_number = u64::from_be_bytes(
            value[..8].try_into().map_err(|_| reth_db_api::DatabaseError::Decode)?,
        );
        let hash = alloy_primitives::B256::from_slice(&value[8..40]);

        Ok(Self(block_number, hash))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;
    use reth_primitives_traits::Account;
    use reth_trie::BranchNodeCompact;

    #[test]
    fn test_maybe_deleted_none() {
        let none: MaybeDeleted<Account> = MaybeDeleted(None);
        let compressed = none.compress();
        assert!(compressed.is_empty(), "None should compress to empty bytes");

        let decompressed = MaybeDeleted::<Account>::decompress(&compressed).unwrap();
        assert_eq!(decompressed.0, None);
    }

    #[test]
    fn test_maybe_deleted_some_account() {
        let account = Account {
            nonce: 42,
            balance: alloy_primitives::U256::from(1000u64),
            bytecode_hash: None,
        };
        let some = MaybeDeleted(Some(account));
        let compressed = some.compress();
        assert!(!compressed.is_empty(), "Some should compress to non-empty bytes");

        let decompressed = MaybeDeleted::<Account>::decompress(&compressed).unwrap();
        assert_eq!(decompressed.0, Some(account));
    }

    #[test]
    fn test_maybe_deleted_some_branch() {
        // Create a simple valid BranchNodeCompact (empty is valid)
        let branch = BranchNodeCompact::new(
            0,      // state_mask
            0,      // tree_mask
            0,      // hash_mask
            vec![], // hashes
            None,   // root_hash
        );
        let some = MaybeDeleted(Some(branch.clone()));
        let compressed = some.compress();
        assert!(!compressed.is_empty(), "Some should compress to non-empty bytes");

        let decompressed = MaybeDeleted::<BranchNodeCompact>::decompress(&compressed).unwrap();
        assert_eq!(decompressed.0, Some(branch));
    }

    #[test]
    fn test_maybe_deleted_roundtrip() {
        let test_cases = vec![
            MaybeDeleted(None),
            MaybeDeleted(Some(Account {
                nonce: 0,
                balance: alloy_primitives::U256::ZERO,
                bytecode_hash: None,
            })),
            MaybeDeleted(Some(Account {
                nonce: 999,
                balance: alloy_primitives::U256::MAX,
                bytecode_hash: Some([0xff; 32].into()),
            })),
        ];

        for original in test_cases {
            let compressed = original.clone().compress();
            let decompressed = MaybeDeleted::<Account>::decompress(&compressed).unwrap();
            assert_eq!(original, decompressed);
        }
    }

    #[test]
    fn test_block_number_hash_roundtrip() {
        let test_cases = vec![
            BlockNumberHash(0, B256::ZERO),
            BlockNumberHash(42, B256::repeat_byte(0xaa)),
            BlockNumberHash(u64::MAX, B256::repeat_byte(0xff)),
        ];

        for original in test_cases {
            let compressed = original.compress();
            let decompressed = BlockNumberHash::decompress(&compressed).unwrap();
            assert_eq!(original, decompressed);
        }
    }
}

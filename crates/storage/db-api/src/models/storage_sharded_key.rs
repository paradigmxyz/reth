//! Storage sharded key
use crate::{
    table::{Decode, Encode},
    DatabaseError,
};
use alloy_primitives::{BlockNumber, B256};
use derive_more::AsRef;
use serde::{Deserialize, Serialize};

use super::ShardedKey;

/// Number of indices in one shard.
pub const NUM_OF_INDICES_IN_SHARD: usize = 2_000;

/// The size of [`StorageShardedKey`] encode bytes.
/// The fields are: 32-byte hashed address, 32-byte key, and 8-byte block number
const STORAGE_SHARD_KEY_BYTES_SIZE: usize = 32 + 32 + 8;

/// Stack-allocated encoded key for `StorageShardedKey`.
///
/// This avoids heap allocation in hot database paths. The key layout is:
/// - 32 bytes: `B256` hashed address
/// - 32 bytes: `B256` storage key
/// - 8 bytes: `BlockNumber` (big-endian)
pub type StorageShardedKeyEncoded = [u8; STORAGE_SHARD_KEY_BYTES_SIZE];

/// Sometimes data can be too big to be saved for a single key. This helps out by dividing the data
/// into different shards. Example:
///
/// `HashedAddress | StorageKey | 200` -> data is from block 0 to 200.
///
/// `HashedAddress | StorageKey | 300` -> data is from block 201 to 300.
#[derive(
    Debug, Default, Clone, Eq, Ord, PartialOrd, PartialEq, AsRef, Serialize, Deserialize, Hash,
)]
pub struct StorageShardedKey {
    /// Hashed storage account address.
    pub hashed_address: B256,
    /// Storage slot with highest block number.
    #[as_ref]
    pub sharded_key: ShardedKey<B256>,
}

impl StorageShardedKey {
    /// Creates a new `StorageShardedKey`.
    pub const fn new(
        hashed_address: B256,
        storage_key: B256,
        highest_block_number: BlockNumber,
    ) -> Self {
        Self { hashed_address, sharded_key: ShardedKey { key: storage_key, highest_block_number } }
    }

    /// Creates a new key with the highest block number set to maximum.
    /// This is useful when we want to search the last value for a given key.
    pub const fn last(hashed_address: B256, storage_key: B256) -> Self {
        Self {
            hashed_address,
            sharded_key: ShardedKey { key: storage_key, highest_block_number: u64::MAX },
        }
    }
}

impl Encode for StorageShardedKey {
    type Encoded = StorageShardedKeyEncoded;

    #[inline]
    fn encode(self) -> Self::Encoded {
        let mut buf = [0u8; STORAGE_SHARD_KEY_BYTES_SIZE];
        buf[..32].copy_from_slice(self.hashed_address.as_slice());
        buf[32..64].copy_from_slice(self.sharded_key.key.as_slice());
        buf[64..].copy_from_slice(&self.sharded_key.highest_block_number.to_be_bytes());
        buf
    }
}

impl Decode for StorageShardedKey {
    fn decode(value: &[u8]) -> Result<Self, DatabaseError> {
        if value.len() != STORAGE_SHARD_KEY_BYTES_SIZE {
            return Err(DatabaseError::Decode)
        }
        let block_num_index = value.len() - 8;

        let highest_block_number = u64::from_be_bytes(
            value[block_num_index..].try_into().map_err(|_| DatabaseError::Decode)?,
        );
        let hashed_address = B256::decode(&value[..32])?;
        let storage_key = B256::decode(&value[32..64])?;

        Ok(Self { hashed_address, sharded_key: ShardedKey::new(storage_key, highest_block_number) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::b256;

    #[test]
    fn storage_sharded_key_encode_decode_roundtrip() {
        let hashed_addr = b256!("0102030405060708091011121314151617181920212223242526272829303132");
        let storage_key = b256!("0001020304050607080910111213141516171819202122232425262728293031");
        let block_num = 0x123456789ABCDEFu64;
        let key = StorageShardedKey::new(hashed_addr, storage_key, block_num);

        let encoded = key.encode();

        // Verify it's stack-allocated (72 bytes)
        assert_eq!(encoded.len(), 72);
        assert_eq!(std::mem::size_of_val(&encoded), 72);

        // Verify roundtrip (check against expected values since key was consumed)
        let decoded = StorageShardedKey::decode(&encoded).unwrap();
        assert_eq!(decoded.hashed_address, b256!("0102030405060708091011121314151617181920212223242526272829303132"));
        assert_eq!(
            decoded.sharded_key.key,
            b256!("0001020304050607080910111213141516171819202122232425262728293031")
        );
        assert_eq!(decoded.sharded_key.highest_block_number, 0x123456789ABCDEFu64);
    }

    #[test]
    fn storage_sharded_key_last_works() {
        let hashed_addr = b256!("0102030405060708091011121314151617181920212223242526272829303132");
        let storage_key = b256!("0001020304050607080910111213141516171819202122232425262728293031");
        let key = StorageShardedKey::last(hashed_addr, storage_key);
        assert_eq!(key.sharded_key.highest_block_number, u64::MAX);

        let encoded = key.encode();
        let decoded = StorageShardedKey::decode(&encoded).unwrap();
        assert_eq!(decoded.sharded_key.highest_block_number, u64::MAX);
    }
}

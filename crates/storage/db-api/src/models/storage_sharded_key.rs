//! Storage sharded key
use crate::{
    table::{Decode, Encode, EncodeInto},
    DatabaseError,
};
use alloy_primitives::{Address, BlockNumber, B256};
use derive_more::AsRef;
use serde::{Deserialize, Serialize};

use super::ShardedKey;

/// Number of indices in one shard.
pub const NUM_OF_INDICES_IN_SHARD: usize = 2_000;

/// The size of [`StorageShardedKey`] encode bytes.
/// The fields are: 20-byte address, 32-byte key, and 8-byte block number
const STORAGE_SHARD_KEY_BYTES_SIZE: usize = 20 + 32 + 8;

/// Stack-allocated encoded key for `StorageShardedKey`.
///
/// This avoids heap allocation in hot database paths. The key layout is:
/// - 20 bytes: `Address`
/// - 32 bytes: `B256` storage key
/// - 8 bytes: `BlockNumber` (big-endian)
pub type StorageShardedKeyEncoded = [u8; STORAGE_SHARD_KEY_BYTES_SIZE];

/// Sometimes data can be too big to be saved for a single key. This helps out by dividing the data
/// into different shards. Example:
///
/// `Address | StorageKey | 200` -> data is from block 0 to 200.
///
/// `Address | StorageKey | 300` -> data is from block 201 to 300.
#[derive(
    Debug, Default, Clone, Eq, Ord, PartialOrd, PartialEq, AsRef, Serialize, Deserialize, Hash,
)]
pub struct StorageShardedKey {
    /// Storage account address.
    pub address: Address,
    /// Storage slot with highest block number.
    #[as_ref]
    pub sharded_key: ShardedKey<B256>,
}

impl StorageShardedKey {
    /// Creates a new `StorageShardedKey`.
    pub const fn new(
        address: Address,
        storage_key: B256,
        highest_block_number: BlockNumber,
    ) -> Self {
        Self { address, sharded_key: ShardedKey { key: storage_key, highest_block_number } }
    }

    /// Creates a new key with the highest block number set to maximum.
    /// This is useful when we want to search the last value for a given key.
    pub const fn last(address: Address, storage_key: B256) -> Self {
        Self {
            address,
            sharded_key: ShardedKey { key: storage_key, highest_block_number: u64::MAX },
        }
    }
}

impl Encode for StorageShardedKey {
    type Encoded = StorageShardedKeyEncoded;

    #[inline]
    fn encode(self) -> Self::Encoded {
        let mut buf = [0u8; STORAGE_SHARD_KEY_BYTES_SIZE];
        buf[..20].copy_from_slice(self.address.as_slice());
        buf[20..52].copy_from_slice(self.sharded_key.key.as_slice());
        buf[52..].copy_from_slice(&self.sharded_key.highest_block_number.to_be_bytes());
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
        let address = Address::decode(&value[..20])?;
        let storage_key = B256::decode(&value[20..52])?;

        Ok(Self { address, sharded_key: ShardedKey::new(storage_key, highest_block_number) })
    }
}

impl EncodeInto for StorageShardedKey {
    #[inline]
    fn encoded_len(&self) -> usize {
        60
    }

    #[inline]
    fn encode_into(&self, buf: &mut [u8]) {
        buf[..20].copy_from_slice(self.address.as_slice());
        buf[20..52].copy_from_slice(self.sharded_key.key.as_slice());
        buf[52..60].copy_from_slice(&self.sharded_key.highest_block_number.to_be_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, b256};

    #[test]
    fn storage_sharded_key_encode_decode_roundtrip() {
        let addr = address!("0102030405060708091011121314151617181920");
        let storage_key = b256!("0001020304050607080910111213141516171819202122232425262728293031");
        let block_num = 0x123456789ABCDEFu64;
        let key = StorageShardedKey::new(addr, storage_key, block_num);

        let encoded = key.encode();

        // Verify it's stack-allocated (60 bytes)
        assert_eq!(encoded.len(), 60);
        assert_eq!(std::mem::size_of_val(&encoded), 60);

        // Verify roundtrip (check against expected values since key was consumed)
        let decoded = StorageShardedKey::decode(&encoded).unwrap();
        assert_eq!(decoded.address, address!("0102030405060708091011121314151617181920"));
        assert_eq!(
            decoded.sharded_key.key,
            b256!("0001020304050607080910111213141516171819202122232425262728293031")
        );
        assert_eq!(decoded.sharded_key.highest_block_number, 0x123456789ABCDEFu64);
    }

    #[test]
    fn storage_sharded_key_last_works() {
        let addr = address!("0102030405060708091011121314151617181920");
        let storage_key = b256!("0001020304050607080910111213141516171819202122232425262728293031");
        let key = StorageShardedKey::last(addr, storage_key);
        assert_eq!(key.sharded_key.highest_block_number, u64::MAX);

        let encoded = key.encode();
        let decoded = StorageShardedKey::decode(&encoded).unwrap();
        assert_eq!(decoded.sharded_key.highest_block_number, u64::MAX);
    }
}

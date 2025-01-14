//! Storage sharded key
use crate::{
    table::{Decode, Encode},
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

/// Sometimes data can be too big to be saved for a single key. This helps out by dividing the data
/// into different shards. Example:
///
/// `Address | StorageKey | 200` -> data is from transition 0 to 200.
///
/// `Address | StorageKey | 300` -> data is from transition 201 to 300.
#[derive(
    Debug, Default, Clone, Eq, Ord, PartialOrd, PartialEq, AsRef, Serialize, Deserialize, Hash,
)]
pub struct StorageShardedKey {
    /// Storage account address.
    pub address: Address,
    /// Storage slot with highest transition id.
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
    type Encoded = Vec<u8>;

    fn encode(self) -> Self::Encoded {
        let mut buf: Vec<u8> = Vec::with_capacity(STORAGE_SHARD_KEY_BYTES_SIZE);
        buf.extend_from_slice(&Encode::encode(self.address));
        buf.extend_from_slice(&Encode::encode(self.sharded_key.key));
        buf.extend_from_slice(&self.sharded_key.highest_block_number.to_be_bytes());
        buf
    }
}

impl Decode for StorageShardedKey {
    fn decode(value: &[u8]) -> Result<Self, DatabaseError> {
        if value.len() != STORAGE_SHARD_KEY_BYTES_SIZE {
            return Err(DatabaseError::Decode)
        }
        let tx_num_index = value.len() - 8;

        let highest_tx_number = u64::from_be_bytes(
            value[tx_num_index..].try_into().map_err(|_| DatabaseError::Decode)?,
        );
        let address = Address::decode(&value[..20])?;
        let storage_key = B256::decode(&value[20..52])?;

        Ok(Self { address, sharded_key: ShardedKey::new(storage_key, highest_tx_number) })
    }
}

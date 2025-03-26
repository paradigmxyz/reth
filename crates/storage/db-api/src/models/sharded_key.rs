//! Sharded key
use crate::{
    table::{Decode, Encode},
    DatabaseError,
};
use alloy_primitives::BlockNumber;
use serde::{Deserialize, Serialize};
use std::hash::Hash;

/// Number of indices in one shard.
pub const NUM_OF_INDICES_IN_SHARD: usize = 2_000;

/// Sometimes data can be too big to be saved for a single key. This helps out by dividing the data
/// into different shards. Example:
///
/// `Address | 200` -> data is from block 0 to 200.
///
/// `Address | 300` -> data is from block 201 to 300.
#[derive(Debug, Default, Clone, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize, Hash)]
pub struct ShardedKey<T> {
    /// The key for this type.
    pub key: T,
    /// Highest block number to which `value` is related to.
    pub highest_block_number: BlockNumber,
}

impl<T> AsRef<Self> for ShardedKey<T> {
    fn as_ref(&self) -> &Self {
        self
    }
}

impl<T> ShardedKey<T> {
    /// Creates a new `ShardedKey<T>`.
    pub const fn new(key: T, highest_block_number: BlockNumber) -> Self {
        Self { key, highest_block_number }
    }

    /// Creates a new key with the highest block number set to maximum.
    /// This is useful when we want to search the last value for a given key.
    pub const fn last(key: T) -> Self {
        Self { key, highest_block_number: u64::MAX }
    }
}

impl<T: Encode> Encode for ShardedKey<T> {
    type Encoded = Vec<u8>;

    fn encode(self) -> Self::Encoded {
        let mut buf: Vec<u8> = Encode::encode(self.key).into();
        buf.extend_from_slice(&self.highest_block_number.to_be_bytes());
        buf
    }
}

impl<T: Decode> Decode for ShardedKey<T> {
    fn decode(value: &[u8]) -> Result<Self, DatabaseError> {
        let (key, highest_tx_number) = value.split_last_chunk().unwrap();
        let key = T::decode(key)?;
        let highest_tx_number = u64::from_be_bytes(*highest_tx_number);
        Ok(Self::new(key, highest_tx_number))
    }
}

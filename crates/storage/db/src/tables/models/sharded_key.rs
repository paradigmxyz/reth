//! Sharded key

use crate::{
    table::{Decode, Encode},
    DatabaseError,
};
use reth_primitives::BlockNumber;
use serde::{Deserialize, Serialize};

/// Number of indices in one shard.
pub const NUM_OF_INDICES_IN_SHARD: usize = 2_000;

/// Sometimes data can be too big to be saved for a single key. This helps out by dividing the data
/// into different shards. Example:
///
/// `Address | 200` -> data is from block 0 to 200.
///
/// `Address | 300` -> data is from block 201 to 300.
#[derive(Debug, Default, Clone, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub struct ShardedKey<T> {
    /// The key for this type.
    pub key: T,
    /// Highest block number to which `value` is related to.
    pub highest_block_number: BlockNumber,
}

impl<T> ShardedKey<T> {
    /// Creates a new `ShardedKey<T>`.
    pub fn new(key: T, highest_block_number: BlockNumber) -> Self {
        ShardedKey { key, highest_block_number }
    }

    /// Creates a new key with the highest block number set to maximum.
    /// This is useful when we want to search the last value for a given key.
    pub fn last(key: T) -> Self {
        Self { key, highest_block_number: u64::MAX }
    }
}

impl<T> Encode for ShardedKey<T>
where
    T: Encode,
    Vec<u8>: From<<T as Encode>::Encoded>,
{
    type Encoded = Vec<u8>;

    fn encode(self) -> Self::Encoded {
        let mut buf: Vec<u8> = Encode::encode(self.key).into();
        buf.extend_from_slice(&self.highest_block_number.to_be_bytes());
        buf
    }
}

impl<T> Decode for ShardedKey<T>
where
    T: Decode,
{
    fn decode<B: AsRef<[u8]>>(value: B) -> Result<Self, DatabaseError> {
        let value = value.as_ref();

        let tx_num_index = value.len() - 8;

        let highest_tx_number = u64::from_be_bytes(
            value[tx_num_index..].try_into().map_err(|_| DatabaseError::DecodeError)?,
        );
        let key = T::decode(&value[..tx_num_index])?;

        Ok(ShardedKey::new(key, highest_tx_number))
    }
}

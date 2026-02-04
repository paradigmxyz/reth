//! Sharded key
use crate::{
    table::{Decode, Encode},
    DatabaseError,
};
use alloy_primitives::{Address, BlockNumber, B256};
use serde::{Deserialize, Serialize};
use std::hash::Hash;

/// Number of indices in one shard.
pub const NUM_OF_INDICES_IN_SHARD: usize = 2_000;

/// Size of `BlockNumber` in bytes (u64 = 8 bytes).
const BLOCK_NUMBER_SIZE: usize = std::mem::size_of::<BlockNumber>();

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

/// Stack-allocated encoded key for `ShardedKey<Address>`.
///
/// This avoids heap allocation in hot database paths. The key layout is:
/// - 20 bytes: `Address`
/// - 8 bytes: `BlockNumber` (big-endian)
pub type ShardedKeyAddressEncoded = [u8; 20 + BLOCK_NUMBER_SIZE];

impl Encode for ShardedKey<Address> {
    type Encoded = ShardedKeyAddressEncoded;

    #[inline]
    fn encode(self) -> Self::Encoded {
        let mut buf = [0u8; 20 + BLOCK_NUMBER_SIZE];
        buf[..20].copy_from_slice(self.key.as_slice());
        buf[20..].copy_from_slice(&self.highest_block_number.to_be_bytes());
        buf
    }
}

impl Decode for ShardedKey<Address> {
    fn decode(value: &[u8]) -> Result<Self, DatabaseError> {
        if value.len() != 20 + BLOCK_NUMBER_SIZE {
            return Err(DatabaseError::Decode);
        }
        let key = Address::from_slice(&value[..20]);
        let highest_block_number =
            u64::from_be_bytes(value[20..].try_into().map_err(|_| DatabaseError::Decode)?);
        Ok(Self::new(key, highest_block_number))
    }
}

/// Stack-allocated encoded key for `ShardedKey<B256>`.
///
/// This avoids heap allocation in hot database paths. The key layout is:
/// - 32 bytes: `B256` (hashed address)
/// - 8 bytes: `BlockNumber` (big-endian)
pub type ShardedKeyB256Encoded = [u8; 32 + BLOCK_NUMBER_SIZE];

impl Encode for ShardedKey<B256> {
    type Encoded = ShardedKeyB256Encoded;

    #[inline]
    fn encode(self) -> Self::Encoded {
        let mut buf = [0u8; 32 + BLOCK_NUMBER_SIZE];
        buf[..32].copy_from_slice(self.key.as_slice());
        buf[32..].copy_from_slice(&self.highest_block_number.to_be_bytes());
        buf
    }
}

impl Decode for ShardedKey<B256> {
    fn decode(value: &[u8]) -> Result<Self, DatabaseError> {
        if value.len() != 32 + BLOCK_NUMBER_SIZE {
            return Err(DatabaseError::Decode);
        }
        let key = B256::from_slice(&value[..32]);
        let highest_block_number =
            u64::from_be_bytes(value[32..].try_into().map_err(|_| DatabaseError::Decode)?);
        Ok(Self::new(key, highest_block_number))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::address;

    #[test]
    fn sharded_key_address_encode_decode_roundtrip() {
        let addr = address!("0102030405060708091011121314151617181920");
        let block_num = 0x123456789ABCDEF0u64;
        let key = ShardedKey::new(addr, block_num);

        let encoded = key.encode();

        // Verify it's stack-allocated (28 bytes)
        assert_eq!(encoded.len(), 28);
        assert_eq!(std::mem::size_of_val(&encoded), 28);

        // Verify roundtrip (check against expected values since key was consumed)
        let decoded = ShardedKey::<Address>::decode(&encoded).unwrap();
        assert_eq!(decoded.key, address!("0102030405060708091011121314151617181920"));
        assert_eq!(decoded.highest_block_number, 0x123456789ABCDEF0u64);
    }

    #[test]
    fn sharded_key_last_works() {
        let addr = address!("0102030405060708091011121314151617181920");
        let key = ShardedKey::<Address>::last(addr);
        assert_eq!(key.highest_block_number, u64::MAX);

        let encoded = key.encode();
        let decoded = ShardedKey::<Address>::decode(&encoded).unwrap();
        assert_eq!(decoded.highest_block_number, u64::MAX);
    }
}

//! Account related models and types.

use crate::{
    impl_fixed_arbitrary,
    table::{Decode, Encode},
    DatabaseError,
};
use alloy_primitives::{Address, BlockNumber, StorageKey, B256};
use serde::{Deserialize, Serialize};
use std::ops::{Bound, Range, RangeBounds, RangeInclusive};

/// [`BlockNumber`] concatenated with [`B256`] (hashed address).
///
/// Since it's used as a key, it isn't compressed when encoding it.
#[derive(
    Debug, Default, Copy, Clone, PartialEq, Eq, Serialize, Deserialize, Ord, PartialOrd, Hash,
)]
pub struct BlockNumberHash(pub (BlockNumber, B256));

impl BlockNumberHash {
    /// Create a new Range from `start` to `end`
    ///
    /// Note: End is inclusive
    pub fn range(range: RangeInclusive<BlockNumber>) -> Range<Self> {
        (*range.start(), B256::ZERO).into()..(*range.end() + 1, B256::ZERO).into()
    }

    /// Return the block number
    pub const fn block_number(&self) -> BlockNumber {
        self.0 .0
    }

    /// Return the hashed address
    pub const fn hashed_address(&self) -> B256 {
        self.0 .1
    }

    /// Consumes `Self` and returns [`BlockNumber`], [`B256`]
    pub const fn take(self) -> (BlockNumber, B256) {
        (self.0 .0, self.0 .1)
    }
}

impl From<(BlockNumber, B256)> for BlockNumberHash {
    fn from(tpl: (u64, B256)) -> Self {
        Self(tpl)
    }
}

impl Encode for BlockNumberHash {
    type Encoded = [u8; 40];

    fn encode(self) -> Self::Encoded {
        let block_number = self.0 .0;
        let hashed_address = self.0 .1;

        let mut buf = [0u8; 40];

        buf[..8].copy_from_slice(&block_number.to_be_bytes());
        buf[8..].copy_from_slice(hashed_address.as_slice());
        buf
    }
}

impl Decode for BlockNumberHash {
    fn decode(value: &[u8]) -> Result<Self, DatabaseError> {
        let num = u64::from_be_bytes(value[..8].try_into().map_err(|_| DatabaseError::Decode)?);
        let hash = B256::from_slice(&value[8..]);
        Ok(Self((num, hash)))
    }
}

/// A [`RangeBounds`] over a range of [`BlockNumberHash`]s. Used to conveniently convert from a
/// range of [`BlockNumber`]s.
#[derive(Debug)]
pub struct BlockNumberHashRange {
    /// Starting bound of the range.
    pub start: Bound<BlockNumberHash>,
    /// Ending bound of the range.
    pub end: Bound<BlockNumberHash>,
}

impl RangeBounds<BlockNumberHash> for BlockNumberHashRange {
    fn start_bound(&self) -> Bound<&BlockNumberHash> {
        self.start.as_ref()
    }

    fn end_bound(&self) -> Bound<&BlockNumberHash> {
        self.end.as_ref()
    }
}

impl<R: RangeBounds<BlockNumber>> From<R> for BlockNumberHashRange {
    fn from(r: R) -> Self {
        let start = match r.start_bound() {
            Bound::Included(n) => Bound::Included(BlockNumberHash((*n, B256::ZERO))),
            Bound::Excluded(n) => Bound::Included(BlockNumberHash((n + 1, B256::ZERO))),
            Bound::Unbounded => Bound::Unbounded,
        };

        let end = match r.end_bound() {
            Bound::Included(n) => Bound::Excluded(BlockNumberHash((n + 1, B256::ZERO))),
            Bound::Excluded(n) => Bound::Excluded(BlockNumberHash((*n, B256::ZERO))),
            Bound::Unbounded => Bound::Unbounded,
        };

        Self { start, end }
    }
}

/// [`Address`] concatenated with [`StorageKey`]. Used by `reth_etl` and history stages.
///
/// Since it's used as a key, it isn't compressed when encoding it.
#[derive(
    Debug, Default, Copy, Clone, PartialEq, Eq, Serialize, Deserialize, Ord, PartialOrd, Hash,
)]
pub struct AddressStorageKey(pub (Address, StorageKey));

impl Encode for AddressStorageKey {
    type Encoded = [u8; 52];

    fn encode(self) -> Self::Encoded {
        let address = self.0 .0;
        let storage_key = self.0 .1;

        let mut buf = [0u8; 52];

        buf[..20].copy_from_slice(address.as_slice());
        buf[20..].copy_from_slice(storage_key.as_slice());
        buf
    }
}

impl Decode for AddressStorageKey {
    fn decode(value: &[u8]) -> Result<Self, DatabaseError> {
        let address = Address::from_slice(&value[..20]);
        let storage_key = StorageKey::from_slice(&value[20..]);
        Ok(Self((address, storage_key)))
    }
}

impl_fixed_arbitrary!((BlockNumberHash, 40), (AddressStorageKey, 52));

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{address, b256};
    use rand::{rng, Rng};

    #[test]
    fn test_block_number_hash() {
        let num = 1u64;
        let hashed_address = b256!("0xba5e000000000000000000000000000000000000000000000000000000000000");
        let key = BlockNumberHash((num, hashed_address));

        let mut bytes = [0u8; 40];
        bytes[..8].copy_from_slice(&num.to_be_bytes());
        bytes[8..].copy_from_slice(hashed_address.as_slice());

        let encoded = Encode::encode(key);
        assert_eq!(encoded, bytes);

        let decoded: BlockNumberHash = Decode::decode(&encoded).unwrap();
        assert_eq!(decoded, key);
    }

    #[test]
    fn test_block_number_hash_rand() {
        let mut bytes = [0u8; 40];
        rng().fill(bytes.as_mut_slice());
        let key = BlockNumberHash::arbitrary(&mut Unstructured::new(&bytes)).unwrap();
        assert_eq!(bytes, Encode::encode(key));
    }

    #[test]
    fn test_address_storage_key() {
        let storage_key = StorageKey::random();
        let address = address!("0xba5e000000000000000000000000000000000000");
        let key = AddressStorageKey((address, storage_key));

        let mut bytes = [0u8; 52];
        bytes[..20].copy_from_slice(address.as_slice());
        bytes[20..].copy_from_slice(storage_key.as_slice());

        let encoded = Encode::encode(key);
        assert_eq!(encoded, bytes);

        let decoded: AddressStorageKey = Decode::decode(&encoded).unwrap();
        assert_eq!(decoded, key);
    }

    #[test]
    fn test_address_storage_key_rand() {
        let mut bytes = [0u8; 52];
        rng().fill(bytes.as_mut_slice());
        let key = AddressStorageKey::arbitrary(&mut Unstructured::new(&bytes)).unwrap();
        assert_eq!(bytes, Encode::encode(key));
    }
}

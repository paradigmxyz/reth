//! Account related models and types.

use std::ops::{Range, RangeInclusive};

use crate::{
    impl_fixed_arbitrary,
    table::{Decode, Encode},
    DatabaseError,
};
use alloy_primitives::{Address, BlockNumber, StorageKey};
use serde::{Deserialize, Serialize};

/// [`BlockNumber`] concatenated with [`Address`].
///
/// Since it's used as a key, it isn't compressed when encoding it.
#[derive(
    Debug, Default, Copy, Clone, PartialEq, Eq, Serialize, Deserialize, Ord, PartialOrd, Hash,
)]
pub struct BlockNumberAddress(pub (BlockNumber, Address));

impl BlockNumberAddress {
    /// Create a new Range from `start` to `end`
    ///
    /// Note: End is inclusive
    pub fn range(range: RangeInclusive<BlockNumber>) -> Range<Self> {
        (*range.start(), Address::ZERO).into()..(*range.end() + 1, Address::ZERO).into()
    }

    /// Return the block number
    pub const fn block_number(&self) -> BlockNumber {
        self.0 .0
    }

    /// Return the address
    pub const fn address(&self) -> Address {
        self.0 .1
    }

    /// Consumes `Self` and returns [`BlockNumber`], [`Address`]
    pub const fn take(self) -> (BlockNumber, Address) {
        (self.0 .0, self.0 .1)
    }
}

impl From<(BlockNumber, Address)> for BlockNumberAddress {
    fn from(tpl: (u64, Address)) -> Self {
        Self(tpl)
    }
}

impl Encode for BlockNumberAddress {
    type Encoded = [u8; 28];

    fn encode(self) -> Self::Encoded {
        let block_number = self.0 .0;
        let address = self.0 .1;

        let mut buf = [0u8; 28];

        buf[..8].copy_from_slice(&block_number.to_be_bytes());
        buf[8..].copy_from_slice(address.as_slice());
        buf
    }
}

impl Decode for BlockNumberAddress {
    fn decode(value: &[u8]) -> Result<Self, DatabaseError> {
        let num = u64::from_be_bytes(value[..8].try_into().map_err(|_| DatabaseError::Decode)?);
        let hash = Address::from_slice(&value[8..]);
        Ok(Self((num, hash)))
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

impl_fixed_arbitrary!((BlockNumberAddress, 28), (AddressStorageKey, 52));

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{thread_rng, Rng};
    use std::str::FromStr;

    #[test]
    fn test_block_number_address() {
        let num = 1u64;
        let hash = Address::from_str("ba5e000000000000000000000000000000000000").unwrap();
        let key = BlockNumberAddress((num, hash));

        let mut bytes = [0u8; 28];
        bytes[..8].copy_from_slice(&num.to_be_bytes());
        bytes[8..].copy_from_slice(hash.as_slice());

        let encoded = Encode::encode(key);
        assert_eq!(encoded, bytes);

        let decoded: BlockNumberAddress = Decode::decode(&encoded).unwrap();
        assert_eq!(decoded, key);
    }

    #[test]
    fn test_block_number_address_rand() {
        let mut bytes = [0u8; 28];
        thread_rng().fill(bytes.as_mut_slice());
        let key = BlockNumberAddress::arbitrary(&mut Unstructured::new(&bytes)).unwrap();
        assert_eq!(bytes, Encode::encode(key));
    }

    #[test]
    fn test_address_storage_key() {
        let storage_key = StorageKey::random();
        let address = Address::from_str("ba5e000000000000000000000000000000000000").unwrap();
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
        thread_rng().fill(bytes.as_mut_slice());
        let key = AddressStorageKey::arbitrary(&mut Unstructured::new(&bytes)).unwrap();
        assert_eq!(bytes, Encode::encode(key));
    }
}

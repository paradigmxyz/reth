//! Table traits.

use super::KVError;
use bytes::Bytes;
use reth_primitives::{Address, U256};
use std::fmt::Debug;

/// Trait that will transform the data to be saved in the DB.
pub trait Encode: Send + Sync + Sized + Debug {
    /// Encoded type.
    type Encoded: AsRef<[u8]> + Send + Sync;

    /// Decodes data going into the database.
    fn encode(self) -> Self::Encoded;
}

/// Trait that will transform the data to be read from the DB.
pub trait Decode: Send + Sync + Sized + Debug {
    /// Decodes data coming from the database.
    fn decode(value: &[u8]) -> Result<Self, KVError>;
}

/// Generic trait that enforces the database value to implement [`Encode`] and [`Decode`].
pub trait Object: Encode + Decode {}

impl<T> Object for T where T: Encode + Decode {}

/// Generic trait that a database table should follow.
pub trait Table: Send + Sync + Debug + 'static {
    /// Key element of `Table`.
    type Key: Encode;
    /// Value element of `Table`.
    type Value: Object;
    /// Seek Key element of `Table`.
    type SeekKey: Encode;

    /// Return name as it is present inside the MDBX.
    fn name(&self) -> &'static str;
}

/// DupSort allows for keys not to be repeated in the database,
/// for more check: https://libmdbx.dqdkfa.ru/usage.html#autotoc_md48
pub trait DupSort: Table {
    /// Subkey type. For more check https://libmdbx.dqdkfa.ru/usage.html#autotoc_md48
    type SubKey: Object;
}

impl Encode for Vec<u8> {
    type Encoded = Self;

    fn encode(self) -> Self::Encoded {
        self
    }
}

impl Decode for Vec<u8> {
    fn decode(value: &[u8]) -> Result<Self, KVError> {
        Ok(value.to_vec())
    }
}

impl Encode for Bytes {
    type Encoded = Self;

    fn encode(self) -> Self::Encoded {
        self
    }
}

impl Decode for Bytes {
    fn decode(value: &[u8]) -> Result<Self, KVError> {
        Ok(value.to_vec().into())
    }
}

impl Encode for Address {
    type Encoded = [u8; 20];

    fn encode(self) -> Self::Encoded {
        self.0
    }
}

impl Decode for Address {
    fn decode(value: &[u8]) -> Result<Self, KVError> {
        Ok(Address::from_slice(value))
    }
}

impl Encode for u16 {
    type Encoded = [u8; 2];

    fn encode(self) -> Self::Encoded {
        self.to_be_bytes()
    }
}

impl Decode for u16 {
    fn decode(value: &[u8]) -> Result<Self, KVError> {
        unsafe { Ok(u16::from_be_bytes(*(value.as_ptr() as *const [_; 2]))) }
    }
}

impl Encode for u64 {
    type Encoded = [u8; 8];

    fn encode(self) -> Self::Encoded {
        self.to_be_bytes()
    }
}

impl Decode for u64 {
    fn decode(value: &[u8]) -> Result<Self, KVError> {
        unsafe { Ok(u64::from_be_bytes(*(value.as_ptr() as *const [_; 8]))) }
    }
}

impl Encode for U256 {
    type Encoded = [u8; 32];

    fn encode(self) -> Self::Encoded {
        let mut result = [0; 32];
        self.to_big_endian(&mut result);
        result
    }
}

impl Decode for U256 {
    fn decode(value: &[u8]) -> Result<Self, KVError> {
        let mut result = [0; 32];
        result.copy_from_slice(value);
        Ok(Self::from_big_endian(&result))
    }
}

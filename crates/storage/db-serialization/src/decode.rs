use crate::DecodeError;
use alloc::{string::String, vec::Vec};
use alloy_primitives::{Address, B256};
use core::fmt::Debug;

/// Trait that will transform the data to be read from the DB.
pub trait Decode: Send + Sync + Sized + Debug {
    /// Decodes data coming from the database.
    fn decode<B: AsRef<[u8]>>(value: B) -> Result<Self, DecodeError>;
}

impl Decode for Vec<u8> {
    fn decode<B: AsRef<[u8]>>(value: B) -> Result<Self, DecodeError> {
        Ok(value.as_ref().to_vec())
    }
}

impl Decode for Address {
    fn decode<B: AsRef<[u8]>>(value: B) -> Result<Self, DecodeError> {
        Ok(Self::from_slice(value.as_ref()))
    }
}

impl Decode for B256 {
    fn decode<B: AsRef<[u8]>>(value: B) -> Result<Self, DecodeError> {
        Ok(Self::new(value.as_ref().try_into().map_err(|_| DecodeError)?))
    }
}

impl Decode for String {
    fn decode<B: AsRef<[u8]>>(value: B) -> Result<Self, DecodeError> {
        Self::from_utf8(value.as_ref().to_vec()).map_err(|_| DecodeError)
    }
}

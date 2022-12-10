//! Implements data structures specific to the database

pub mod accounts;
pub mod blocks;
pub mod integer_list;
pub mod sharded_key;

pub use accounts::*;
pub use blocks::*;
use reth_primitives::{Address, H256};
pub use sharded_key::ShardedKey;

use crate::{
    table::{Decode, Encode},
    Error,
};

/// Macro that implements [`Encode`] and [`Decode`] for uint types.
macro_rules! impl_uints {
    ($($name:tt),+) => {
        $(
            impl Encode for $name
            {
                type Encoded = [u8; std::mem::size_of::<$name>()];

                fn encode(self) -> Self::Encoded {
                    self.to_be_bytes()
                }
            }

            impl Decode for $name
            {
                fn decode<B: Into<bytes::Bytes>>(value: B) -> Result<Self, Error> {
                    let value: bytes::Bytes = value.into();
                    Ok(
                        $name::from_be_bytes(
                            value.as_ref().try_into().map_err(|_| Error::DecodeError)?
                        )
                    )
                }
            }
        )+
    };
}

impl_uints!(u64, u32, u16, u8);

impl Encode for Vec<u8> {
    type Encoded = Vec<u8>;
    fn encode(self) -> Self::Encoded {
        self
    }
}

impl Decode for Vec<u8> {
    fn decode<B: Into<bytes::Bytes>>(value: B) -> Result<Self, Error> {
        Ok(value.into().to_vec())
    }
}

impl Encode for Address {
    type Encoded = [u8; 20];
    fn encode(self) -> Self::Encoded {
        self.to_fixed_bytes()
    }
}

impl Decode for Address {
    fn decode<B: Into<bytes::Bytes>>(value: B) -> Result<Self, Error> {
        Ok(Address::from_slice(&value.into()[..]))
    }
}
impl Encode for H256 {
    type Encoded = [u8; 32];
    fn encode(self) -> Self::Encoded {
        self.to_fixed_bytes()
    }
}

impl Decode for H256 {
    fn decode<B: Into<bytes::Bytes>>(value: B) -> Result<Self, Error> {
        Ok(H256::from_slice(&value.into()[..]))
    }
}

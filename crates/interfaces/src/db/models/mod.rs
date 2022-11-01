//! Implements data structures specific to the database

pub mod accounts;
pub mod blocks;
pub mod integer_list;
pub mod sharded_key;

pub use accounts::*;
pub use blocks::*;
pub use sharded_key::ShardedKey;

use crate::db::{
    table::{Decode, Encode},
    Error,
};
use eyre::eyre;

/// Macro that implements [`Encode`] and [`Decode`] for uint types.
macro_rules! impl_uints {
    ($(($name:tt, $size:tt)),+) => {
        $(
            impl Encode for $name
            {
                type Encoded = [u8; $size];

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
                            value.as_ref().try_into().map_err(|_| Error::Decode(eyre!("Into bytes error.")))?
                        )
                    )
                }
            }
        )+
    };
}

impl_uints!((u64, 8), (u32, 4), (u16, 2), (u8, 1));

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

/// Helper type to ensure that `uint` keys are saved in raw and big-endian format.
#[derive(Debug)]
pub struct UncompressedUint<T> {
    /// Inner value
    pub inner: T,
}

/// Macro to ensure that UncompressedUint<uintN> keys are saved in raw and big-endian format.
macro_rules! impl_uints {
    ($(($name:tt, $size:tt)),+) => {
        $(
            impl Encode for UncompressedUint<$name>
            {
                type Encoded = [u8; $size];

                fn encode(self) -> Self::Encoded {
                    $name::to_be_bytes(self.inner)
                }
            }

            impl Decode for UncompressedUint<$name>
            {
                fn decode<B: Into<bytes::Bytes>>(value: B) -> Result<Self, Error> {
                    let value: bytes::Bytes = value.into();
                    Ok(UncompressedUint { inner: $name::from_be_bytes(
                         value.as_ref().try_into().map_err(|_| Error::Decode(eyre!("Into bytes error.")))?
                    ) })
                }
            }

            impl From<$name> for UncompressedUint<$name> {
                fn from(num: $name) -> Self {
                    UncompressedUint {inner: num}
                }
            }

            impl From<UncompressedUint<$name>> for $name {
                fn from(num: UncompressedUint<$name>) -> Self {
                    num.inner
                }
            }
        )+
    };
}

impl_uints!((u64, 8), (u32, 4), (u16, 2), (u8, 1));

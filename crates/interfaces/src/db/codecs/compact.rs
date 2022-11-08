use crate::db::{models::accounts::AccountBeforeTx, Decode, Encode, Error};
use reth_codecs::Compact;
use reth_primitives::*;

macro_rules! impl_compact {
    ($($name:tt),+) => {
        $(
            impl Encode for $name
            {
                type Encoded = Vec<u8>;

                fn encode(self) -> Self::Encoded {
                    let (_, obj) = Compact::to_compact(self);
                    obj
                }
            }

            impl Decode for $name
            {
                fn decode<B: Into<bytes::Bytes>>(value: B) -> Result<$name, Error> {
                    let value = value.into();
                    let (obj, _) = Compact::from_compact(&value, value.len());
                    Ok(obj)
                }
            }
        )+
    };
}

impl_compact!(Header, Account);

#![allow(unused)]

use crate::{
    table::{Decode, Encode},
    DatabaseError,
};
use postcard::{from_bytes, to_allocvec, to_vec};
use reth_primitives::*;

// Just add `Serialize` and `Deserialize`, and set impl_heapless_postcard!(T, MaxSize(T))
//
//
// use serde::{Deserialize, Serialize};
//
// #[derive(Serialize, Deserialize )]
// pub struct T {
// }
//
// impl_heapless_postcard!(T, MaxSize(T))
macro_rules! impl_postcard {
    ($($name:tt),+) => {
        $(
            impl Encode for $name {
                type Encoded = Vec<u8>;

                fn encode(self) -> Self::Encoded {
                    to_allocvec(&self).expect("Failed to encode")
                }
            }

            impl Decode for $name {
                fn decode<B: Into<bytes::Bytes>>(value: B) -> Result<Self, Error> {
                    from_bytes(&value.into()).map_err(|e| Error::Decode(e.into()))
                }
            }
        )+
    };
}

type VecU8 = Vec<u8>;

//#[cfg(feature = "bench-postcard")]
//impl_postcard!(VecU8, Receipt, H256, U256, H160, u8, u16, u64, Header, Account, Log, TxType);

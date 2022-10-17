#![allow(unused)]

use crate::kv::{Decode, Encode, KVError};
use heapless::Vec;
use postcard::{from_bytes, to_vec};
use reth_primitives::Account;

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

macro_rules! impl_heapless_postcard {
    ($name:tt, $static_size:tt) => {
        impl Encode for $name {
            type Encoded = Vec<u8, $static_size>;

            fn encode(self) -> Self::Encoded {
                to_vec(&self).expect("Failed to encode")
            }
        }

        impl Decode for $name {
            fn decode<B: Into<bytes::Bytes>>(value: B) -> Result<Self, KVError> {
                from_bytes(&value.into()).map_err(|_| KVError::InvalidValue)
            }
        }
    };
}

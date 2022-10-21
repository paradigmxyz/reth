//! Implements [`Encode`] and [`Decode`] for [`IntegerList`]

use crate::db::{
    error::Error,
    table::{Decode, Encode},
};
use bytes::Bytes;
use reth_primitives::IntegerList;

impl Encode for IntegerList {
    type Encoded = Vec<u8>;

    fn encode(self) -> Self::Encoded {
        self.to_bytes()
    }
}

impl Decode for IntegerList {
    fn decode<B: Into<Bytes>>(value: B) -> Result<Self, Error> {
        IntegerList::from_bytes(&value.into()).map_err(|e| Error::Decode(eyre::eyre!("{e}")))
    }
}

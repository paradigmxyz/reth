//! Implements [`Compress`] and [`Decompress`] for [`IntegerList`]

use crate::db::{
    error::Error,
    table::{Compress, Decompress},
};
use bytes::Bytes;
use reth_primitives::IntegerList;

impl Compress for IntegerList {
    type Compressed = Vec<u8>;

    fn compress(self) -> Self::Compressed {
        self.to_bytes()
    }
}

impl Decompress for IntegerList {
    fn decompress<B: Into<Bytes>>(value: B) -> Result<Self, Error> {
        IntegerList::from_bytes(&value.into()).map_err(|e| Error::Decode(eyre::eyre!("{e}")))
    }
}

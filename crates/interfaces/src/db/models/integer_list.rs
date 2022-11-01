//! Implements [`Compress`] and [`Uncompress`] for [`IntegerList`]

use crate::db::{
    error::Error,
    table::{Compress, Uncompress},
};
use bytes::Bytes;
use reth_primitives::IntegerList;

impl Compress for IntegerList {
    type Compressed = Vec<u8>;

    fn compress(self) -> Self::Compressed {
        self.to_bytes()
    }
}

impl Uncompress for IntegerList {
    fn uncompress<B: Into<Bytes>>(value: B) -> Result<Self, Error> {
        IntegerList::from_bytes(&value.into()).map_err(|e| Error::Decode(eyre::eyre!("{e}")))
    }
}

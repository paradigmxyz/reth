//! Implements [`Compress`] and [`Decompress`] for [`IntegerList`]

use crate::{
    table::{Compress, Decompress},
    Error,
};
use reth_primitives::IntegerList;

impl Compress for IntegerList {
    type Compressed = Vec<u8>;

    fn compress(self) -> Self::Compressed {
        self.to_bytes()
    }
    fn compress_to_buf<B: bytes::BufMut + AsMut<[u8]>>(self, buf: &mut B) {
        self.to_mut_bytes(buf)
    }
}

impl Decompress for IntegerList {
    fn decompress<B: AsRef<[u8]>>(value: B) -> Result<Self, Error> {
        IntegerList::from_bytes(value.as_ref()).map_err(|_| Error::DecodeError)
    }
}

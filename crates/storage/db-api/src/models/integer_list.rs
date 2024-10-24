//! Implements [`Compress`] and [`Decompress`] for [`IntegerList`]

use crate::{
    table::{Compress, Decompress},
    DatabaseError,
};
use reth_primitives_traits::IntegerList;

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
    fn decompress(value: &[u8]) -> Result<Self, DatabaseError> {
        Self::from_bytes(value).map_err(|_| DatabaseError::Decode)
    }
}

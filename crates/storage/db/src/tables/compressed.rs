use crate::{
    table::{Compress, Decompress, Table, Value},
    Error,
};
use serde::Serialize;

/// Raw table that can be used to access any table and its data in raw mode.
/// This is useful for delayed decoding/encoding of data.
#[derive(Default, Copy, Clone, Debug)]
pub struct CompressedTable<T: Table> {
    phantom: std::marker::PhantomData<T>,
}

impl<T: Table> Table for CompressedTable<T> {
    const NAME: &'static str = T::NAME;

    type Key = T::Key;

    type Value = CompressedValue<T::Value>;
}



/// Raw table value.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Serialize, Ord, Hash)]
pub struct CompressedValue<V: Value> {
    value: Vec<u8>,
    _phantom: std::marker::PhantomData<V>,
}

impl<V: Value> CompressedValue<V> {
    /// Create new raw value.
    pub fn new(value: V) -> Self {
        let compressed = zstd::bulk::compress(V::compress(value).as_ref(), 3).unwrap();
        Self { value: compressed, _phantom: std::marker::PhantomData }
    }
    /// Returns the raw value.
    pub fn value(&self) -> Result<V, Error> {
        // capacity is 3 times the compressed size.
        let decompressed = zstd::bulk::decompress(&self.value,self.value.len()*3).unwrap();
        V::decompress(&decompressed)
    }
}

impl AsRef<[u8]> for CompressedValue<Vec<u8>> {
    fn as_ref(&self) -> &[u8] {
        &self.value
    }
}

impl<V: Value> Compress for CompressedValue<V> {
    type Compressed = Vec<u8>;

    fn compress(self) -> Self::Compressed {
        self.value
    }

    fn compress_to_buf<B: bytes::BufMut + AsMut<[u8]>>(self, buf: &mut B) {
        buf.put_slice(self.value.as_slice())
    }

    fn uncompressable_ref(&self) -> Option<&[u8]> {
        // Already compressed
        Some(&self.value)
    }
}

impl<V: Value> Decompress for CompressedValue<V> {
    fn decompress<B: AsRef<[u8]>>(value: B) -> Result<Self, Error> {
        Ok(Self { value: value.as_ref().to_vec(), _phantom: std::marker::PhantomData })
    }
}
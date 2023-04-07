use crate::{
    table::{Compress, Decode, Decompress, DupSort, Encode, Key, Table, Value},
    Error,
};
use serde::Serialize;

/// Raw table that can be used to access any table and its data in raw mode.
/// This is useful for delayed decoding/encoding of data.
#[derive(Default, Copy, Clone, Debug)]
pub struct RawTable<T: Table> {
    phantom: std::marker::PhantomData<T>,
}

impl<T: Table> Table for RawTable<T> {
    const NAME: &'static str = T::NAME;

    type Key = RawKey<T::Key>;

    type Value = RawValue<T::Value>;
}

/// Raw DubSort table that can be used to access any table and its data in raw mode.
/// This is useful for delayed decoding/encoding of data.
#[derive(Default, Copy, Clone, Debug)]
pub struct RawDubSort<T: DupSort> {
    phantom: std::marker::PhantomData<T>,
}

impl<T: DupSort> Table for RawDubSort<T> {
    const NAME: &'static str = T::NAME;

    type Key = RawKey<T::Key>;

    type Value = RawValue<T::Value>;
}

impl<T: DupSort> DupSort for RawDubSort<T> {
    type SubKey = RawKey<T::SubKey>;
}

/// Raw table key.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RawKey<K: Key> {
    key: Vec<u8>,
    _phantom: std::marker::PhantomData<K>,
}

impl<K: Key> RawKey<K> {
    /// Create new raw key.
    pub fn new(key: K) -> Self {
        Self { key: K::encode(key).as_ref().to_vec(), _phantom: std::marker::PhantomData }
    }
    /// Returns the raw key.
    pub fn key(&self) -> Result<K, Error> {
        K::decode(&self.key)
    }
}

impl AsRef<[u8]> for RawKey<Vec<u8>> {
    fn as_ref(&self) -> &[u8] {
        &self.key
    }
}

// Encode
impl<K: Key> Encode for RawKey<K> {
    type Encoded = Vec<u8>;

    fn encode(self) -> Self::Encoded {
        self.key
    }
}

// Decode
impl<K: Key> Decode for RawKey<K> {
    fn decode<B: AsRef<[u8]>>(key: B) -> Result<Self, Error> {
        Ok(Self { key: key.as_ref().to_vec(), _phantom: std::marker::PhantomData })
    }
}

/// Raw table value.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Serialize, Ord, Hash)]
pub struct RawValue<V: Value> {
    value: Vec<u8>,
    _phantom: std::marker::PhantomData<V>,
}

impl<V: Value> RawValue<V> {
    /// Create new raw value.
    pub fn new(value: V) -> Self {
        Self { value: V::compress(value).as_ref().to_vec(), _phantom: std::marker::PhantomData }
    }
    /// Returns the raw value.
    pub fn value(&self) -> Result<V, Error> {
        V::decompress(&self.value)
    }
}

impl AsRef<[u8]> for RawValue<Vec<u8>> {
    fn as_ref(&self) -> &[u8] {
        &self.value
    }
}

impl<V: Value> Compress for RawValue<V> {
    type Compressed = Vec<u8>;

    fn compress(self) -> Self::Compressed {
        self.value
    }
}

impl<V: Value> Decompress for RawValue<V> {
    fn decompress<B: AsRef<[u8]>>(value: B) -> Result<Self, Error> {
        Ok(Self { value: value.as_ref().to_vec(), _phantom: std::marker::PhantomData })
    }
}

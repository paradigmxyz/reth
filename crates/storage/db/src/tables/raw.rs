use crate::{
    table::{Compress, Decode, Decompress, DupSort, Encode, Key, Table, Value},
    DatabaseError,
};
use serde::{Deserialize, Serialize};

/// Tuple with `RawKey<T::Key>` and `RawValue<T::Value>`.
pub type TableRawRow<T> = (RawKey<<T as Table>::Key>, RawValue<<T as Table>::Value>);

/// Raw table that can be used to access any table and its data in raw mode.
/// This is useful for delayed decoding/encoding of data.
#[derive(Default, Copy, Clone, Debug)]
pub struct RawTable<T: Table> {
    phantom: std::marker::PhantomData<T>,
}

impl<T: Table> Table for RawTable<T> {
    const TABLE: crate::Tables = T::TABLE;

    type Key = RawKey<T::Key>;
    type Value = RawValue<T::Value>;
}

/// Raw DupSort table that can be used to access any table and its data in raw mode.
/// This is useful for delayed decoding/encoding of data.
#[derive(Default, Copy, Clone, Debug)]
pub struct RawDupSort<T: DupSort> {
    phantom: std::marker::PhantomData<T>,
}

impl<T: DupSort> Table for RawDupSort<T> {
    const TABLE: crate::Tables = T::TABLE;

    type Key = RawKey<T::Key>;
    type Value = RawValue<T::Value>;
}

impl<T: DupSort> DupSort for RawDupSort<T> {
    type SubKey = RawKey<T::SubKey>;
}

/// Raw table key.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RawKey<K: Key> {
    /// Inner encoded key
    key: Vec<u8>,
    _phantom: std::marker::PhantomData<K>,
}

impl<K: Key> RawKey<K> {
    /// Create new raw key.
    pub fn new(key: K) -> Self {
        Self { key: K::encode(key).into(), _phantom: std::marker::PhantomData }
    }

    /// Creates a raw key from an existing `Vec`. Useful when we already have the encoded
    /// key.
    pub fn from_vec(vec: Vec<u8>) -> Self {
        Self { key: vec, _phantom: std::marker::PhantomData }
    }

    /// Returns the decoded value.
    pub fn key(&self) -> Result<K, DatabaseError> {
        K::decode(&self.key)
    }

    /// Returns the raw key as seen on the database.
    pub fn raw_key(&self) -> &Vec<u8> {
        &self.key
    }

    /// Consumes [`Self`] and returns the inner raw key.
    pub fn into_key(self) -> Vec<u8> {
        self.key
    }
}

impl<K: Key> From<K> for RawKey<K> {
    fn from(key: K) -> Self {
        RawKey::new(key)
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
    fn decode<B: AsRef<[u8]>>(key: B) -> Result<Self, DatabaseError> {
        Ok(Self { key: key.as_ref().to_vec(), _phantom: std::marker::PhantomData })
    }
}

/// Raw table value.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Serialize, Ord, Hash)]
pub struct RawValue<V: Value> {
    /// Inner compressed value
    value: Vec<u8>,
    #[serde(skip)]
    _phantom: std::marker::PhantomData<V>,
}

impl<V: Value> RawValue<V> {
    /// Create new raw value.
    pub fn new(value: V) -> Self {
        Self { value: V::compress(value).into(), _phantom: std::marker::PhantomData }
    }

    /// Creates a raw value from an existing `Vec`. Useful when we already have the encoded
    /// value.
    pub fn from_vec(vec: Vec<u8>) -> Self {
        Self { value: vec, _phantom: std::marker::PhantomData }
    }

    /// Returns the decompressed value.
    pub fn value(&self) -> Result<V, DatabaseError> {
        V::decompress(&self.value)
    }

    /// Returns the raw value as seen on the database.
    pub fn raw_value(&self) -> &[u8] {
        &self.value
    }

    /// Consumes [`Self`] and returns the inner raw value.
    pub fn into_value(self) -> Vec<u8> {
        self.value
    }
}

impl<V: Value> From<V> for RawValue<V> {
    fn from(value: V) -> Self {
        RawValue::new(value)
    }
}

impl AsRef<[u8]> for RawValue<Vec<u8>> {
    fn as_ref(&self) -> &[u8] {
        &self.value
    }
}

impl<V: Value> Compress for RawValue<V> {
    type Compressed = Vec<u8>;

    fn uncompressable_ref(&self) -> Option<&[u8]> {
        // Already compressed
        Some(&self.value)
    }

    fn compress(self) -> Self::Compressed {
        self.value
    }

    fn compress_to_buf<B: bytes::BufMut + AsMut<[u8]>>(self, buf: &mut B) {
        buf.put_slice(self.value.as_slice())
    }
}

impl<V: Value> Decompress for RawValue<V> {
    fn decompress<B: AsRef<[u8]>>(value: B) -> Result<Self, DatabaseError> {
        Ok(Self { value: value.as_ref().to_vec(), _phantom: std::marker::PhantomData })
    }

    fn decompress_owned(value: Vec<u8>) -> Result<Self, DatabaseError> {
        Ok(Self { value, _phantom: std::marker::PhantomData })
    }
}

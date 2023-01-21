use crate::Error;
use bytes::Bytes;
use serde::Serialize;
use std::{
    fmt::Debug,
    marker::{Send, Sync},
};

/// Trait that will transform the data to be saved in the DB in a (ideally) compressed format
pub trait Compress: Send + Sync + Sized + Debug {
    /// Compressed type.
    type Compressed: AsRef<[u8]> + Send + Sync;

    /// Compresses data going into the database.
    fn compress(self) -> Self::Compressed;
}

/// Trait that will transform the data to be read from the DB.
pub trait Decompress: Send + Sync + Sized + Debug {
    /// Decompresses data coming from the database.
    fn decompress<B: Into<Bytes>>(value: B) -> Result<Self, Error>;
}

/// Trait that will transform the data to be saved in the DB.
pub trait Encode: Send + Sync + Sized + Debug {
    /// Encoded type.
    type Encoded: AsRef<[u8]> + Send + Sync;

    /// Encodes data going into the database.
    fn encode(self) -> Self::Encoded;
}

/// Trait that will transform the data to be read from the DB.
pub trait Decode: Send + Sync + Sized + Debug {
    /// Decodes data coming from the database.
    fn decode<B: Into<Bytes>>(value: B) -> Result<Self, Error>;
}

/// Generic trait that enforces the database key to implement [`Encode`] and [`Decode`].
pub trait Key: Encode + Decode + Ord + Clone {}

impl<T> Key for T where T: Encode + Decode + Ord + Clone {}

/// Generic trait that enforces the database value to implement [`Compress`] and [`Decompress`].
pub trait Value: Compress + Decompress + Serialize {}

impl<T> Value for T where T: Compress + Decompress + Serialize {}

/// Generic trait that a database table should follow.
///
/// [`Table::Key`], [`Table::Value`], [`Table::SeekKey`] types should implement [`Encode`] and
/// [`Decode`] when appropriate. These traits define how the data is stored and read from the
/// database.
///
/// It allows for the use of codecs. See [`crate::kv::models::blocks::BlockNumHash`] for a custom
/// implementation, and [`crate::kv::codecs::scale`] for the use of an external codec.
pub trait Table: Send + Sync + Debug + 'static {
    /// Return table name as it is present inside the MDBX.
    const NAME: &'static str;
    /// Key element of `Table`.
    ///
    /// Sorting should be taken into account when encoding this.
    type Key: Key;
    /// Value element of `Table`.
    type Value: Value;
}

/// DupSort allows for keys not to be repeated in the database,
/// for more check: https://libmdbx.dqdkfa.ru/usage.html#autotoc_md48
pub trait DupSort: Table {
    /// Subkey type. For more check https://libmdbx.dqdkfa.ru/usage.html#autotoc_md48
    ///
    /// Sorting should be taken into account when encoding this.
    type SubKey: Key;
}

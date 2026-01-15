use crate::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
    transaction::{DbTx, DbTxMut},
    DatabaseError,
};

use serde::{Deserialize, Serialize};
use std::fmt::Debug;

/// Maximum size of a key buffer for stack-based serialization.
/// This accommodates the largest keys used in reth tables (e.g., `ShardedKey<B256>` = 32 + 8 = 40
/// bytes, `AddressStorageKey` = 52 bytes).
pub const MAX_KEY_SIZE: usize = 64;

/// Error type for deserialization failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeserError;

/// Trait for zero-copy key serialization.
///
/// Keys are serialized directly to a stack buffer to avoid heap allocations.
/// All implementors must have a fixed, known size at compile time.
pub trait KeySer: Ord + Sized + Send + Sync + Debug {
    /// The exact size in bytes of the encoded key.
    const SIZE: usize;

    /// Compile-time assertion that SIZE is valid.
    const ASSERT: () = {
        assert!(Self::SIZE <= MAX_KEY_SIZE, "Key size exceeds MAX_KEY_SIZE");
        assert!(Self::SIZE > 0, "Key size must be non-zero");
    };

    /// Encodes the key into the provided buffer, returning a slice of the encoded bytes.
    ///
    /// The returned slice borrows from either `self` or `buf`, depending on the implementation.
    fn encode_key<'a: 'c, 'b: 'c, 'c>(&'a self, buf: &'b mut [u8; MAX_KEY_SIZE]) -> &'c [u8];

    /// Decodes a key from the given byte slice.
    fn decode_key(data: &[u8]) -> Result<Self, DeserError>;
}

/// Trait for zero-copy value serialization.
///
/// Values report their encoded size upfront, allowing the caller to reserve exactly
/// the right amount of space in MDBX before serializing directly into that buffer.
pub trait ValSer: Send + Sync + Debug {
    /// Returns the exact size in bytes that will be written by `encode_value_to`.
    fn encoded_size(&self) -> usize;

    /// Encodes the value into the provided buffer.
    ///
    /// The buffer must have at least `encoded_size()` bytes available.
    fn encode_value_to<B>(&self, buf: &mut B)
    where
        B: bytes::BufMut + AsMut<[u8]>;

    /// Decodes a value from the given byte slice.
    fn decode_value(data: &[u8]) -> Result<Self, DeserError>
    where
        Self: Sized;
}

/// Trait that will transform the data to be saved in the DB in a (ideally) compressed format
pub trait Compress: Send + Sync + Sized + Debug {
    /// Compressed type.
    type Compressed: bytes::BufMut
        + AsRef<[u8]>
        + AsMut<[u8]>
        + Into<Vec<u8>>
        + Default
        + Send
        + Sync
        + Debug;

    /// If the type cannot be compressed, return its inner reference as `Some(self.as_ref())`
    fn uncompressable_ref(&self) -> Option<&[u8]> {
        None
    }

    /// Compresses data going into the database.
    fn compress(self) -> Self::Compressed {
        let mut buf = Self::Compressed::default();
        self.compress_to_buf(&mut buf);
        buf
    }

    /// Compresses data to a given buffer.
    fn compress_to_buf<B: bytes::BufMut + AsMut<[u8]>>(&self, buf: &mut B);
}

/// Trait that will transform the data to be read from the DB.
pub trait Decompress: Send + Sync + Sized + Debug {
    /// Decompresses data coming from the database.
    fn decompress(value: &[u8]) -> Result<Self, DatabaseError>;

    /// Decompresses owned data coming from the database.
    fn decompress_owned(value: Vec<u8>) -> Result<Self, DatabaseError> {
        Self::decompress(&value)
    }
}

/// Trait that will transform the data to be saved in the DB.
pub trait Encode: Send + Sync + Sized + Debug {
    /// Encoded type.
    type Encoded: AsRef<[u8]> + Into<Vec<u8>> + Send + Sync + Ord + Debug;

    /// Encodes data going into the database.
    fn encode(self) -> Self::Encoded;
}

/// Trait that will transform the data to be read from the DB.
pub trait Decode: Send + Sync + Sized + Debug {
    /// Decodes data coming from the database.
    fn decode(value: &[u8]) -> Result<Self, DatabaseError>;

    /// Decodes owned data coming from the database.
    fn decode_owned(value: Vec<u8>) -> Result<Self, DatabaseError> {
        Self::decode(&value)
    }
}

/// Generic trait that enforces the database key to implement [`Encode`] and [`Decode`].
pub trait Key: Encode + Decode + Ord + Clone + Serialize + for<'a> Deserialize<'a> {}

impl<T> Key for T where T: Encode + Decode + Ord + Clone + Serialize + for<'a> Deserialize<'a> {}

/// Generic trait that enforces the database value to implement [`Compress`] and [`Decompress`].
pub trait Value: Compress + Decompress + Serialize {}

impl<T> Value for T where T: Compress + Decompress + Serialize {}

/// Generic trait that a database table should follow.
///
/// The [`Table::Key`] and [`Table::Value`] types should implement [`Encode`] and
/// [`Decode`] when appropriate. These traits define how the data is stored and read from the
/// database.
///
/// It allows for the use of codecs. See [`crate::models::ShardedKey`] for a custom
/// implementation.
pub trait Table: Send + Sync + Debug + 'static {
    /// The table's name.
    const NAME: &'static str;

    /// Whether the table is also a `DUPSORT` table.
    const DUPSORT: bool;

    /// Key element of `Table`.
    ///
    /// Sorting should be taken into account when encoding this.
    type Key: Key;

    /// Value element of `Table`.
    type Value: Value;
}

/// Trait that provides object-safe access to the table's metadata.
pub trait TableInfo: Send + Sync + Debug + 'static {
    /// The table's name.
    fn name(&self) -> &'static str;

    /// Whether the table is a `DUPSORT` table.
    fn is_dupsort(&self) -> bool;
}

/// Tuple with `T::Key` and `T::Value`.
pub type TableRow<T> = (<T as Table>::Key, <T as Table>::Value);

/// `DupSort` allows for keys to be repeated in the database.
///
/// Upstream docs: <https://libmdbx.dqdkfa.ru/usage.html#autotoc_md48>
pub trait DupSort: Table {
    /// The table subkey. This type must implement [`Encode`] and [`Decode`].
    ///
    /// Sorting should be taken into account when encoding this.
    ///
    /// Upstream docs: <https://libmdbx.dqdkfa.ru/usage.html#autotoc_md48>
    type SubKey: Key;
}

/// Allows duplicating tables across databases
pub trait TableImporter: DbTxMut {
    /// Imports all table data from another transaction.
    fn import_table<T: Table, R: DbTx>(&self, source_tx: &R) -> Result<(), DatabaseError> {
        let mut destination_cursor = self.cursor_write::<T>()?;

        for kv in source_tx.cursor_read::<T>()?.walk(None)? {
            let (k, v) = kv?;
            destination_cursor.append(k, &v)?;
        }

        Ok(())
    }

    /// Imports table data from another transaction within a range.
    ///
    /// This method works correctly with both regular and `DupSort` tables. For `DupSort` tables,
    /// all duplicate entries within the range are preserved during import.
    fn import_table_with_range<T: Table, R: DbTx>(
        &self,
        source_tx: &R,
        from: Option<<T as Table>::Key>,
        to: <T as Table>::Key,
    ) -> Result<(), DatabaseError>
    where
        T::Key: Default,
    {
        let mut destination_cursor = self.cursor_write::<T>()?;
        let mut source_cursor = source_tx.cursor_read::<T>()?;

        let source_range = match from {
            Some(from) => source_cursor.walk_range(from..=to),
            None => source_cursor.walk_range(..=to),
        };
        for row in source_range? {
            let (key, value) = row?;
            destination_cursor.append(key, &value)?;
        }

        Ok(())
    }

    /// Imports all dupsort data from another transaction.
    fn import_dupsort<T: DupSort, R: DbTx>(&self, source_tx: &R) -> Result<(), DatabaseError> {
        let mut destination_cursor = self.cursor_dup_write::<T>()?;
        let mut cursor = source_tx.cursor_dup_read::<T>()?;

        while let Some((k, _)) = cursor.next_no_dup()? {
            for kv in cursor.walk_dup(Some(k), None)? {
                let (k, v) = kv?;
                destination_cursor.append_dup(k, v)?;
            }
        }

        Ok(())
    }
}

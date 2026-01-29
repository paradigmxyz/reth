use crate::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
    transaction::{DbTx, DbTxMut},
    DatabaseError,
};

use serde::{Deserialize, Serialize};
use std::fmt::Debug;

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

/// Trait for converting encoded types to `Vec<u8>`.
///
/// This is implemented for all `AsRef<[u8]>` types. For `Vec<u8>` this is a no-op,
/// for other types like `ArrayVec` or fixed arrays it performs a copy.
pub trait IntoVec: AsRef<[u8]> {
    /// Convert to a `Vec<u8>`.
    fn into_vec(self) -> Vec<u8>;
}

impl IntoVec for Vec<u8> {
    #[inline]
    fn into_vec(self) -> Vec<u8> {
        self
    }
}

impl<const N: usize> IntoVec for [u8; N] {
    #[inline]
    fn into_vec(self) -> Vec<u8> {
        self.to_vec()
    }
}

impl<const N: usize> IntoVec for arrayvec::ArrayVec<u8, N> {
    #[inline]
    fn into_vec(self) -> Vec<u8> {
        self.to_vec()
    }
}

/// Trait that will transform the data to be saved in the DB.
pub trait Encode: Send + Sync + Sized + Debug {
    /// Encoded type.
    type Encoded: AsRef<[u8]> + IntoVec + Send + Sync + Ord + Debug;

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

/// Trait for zero-copy encoding directly into a provided buffer.
///
/// This enables MDBX_RESERVE-style writes where we write directly into
/// MDBX's memory-mapped pages, avoiding intermediate allocations.
///
/// For types where the in-memory representation equals the encoded bytes
/// (e.g., `Address`, `B256`), implement this trait directly to avoid
/// any intermediate copies. For other fixed-size types, use the blanket
/// impl via [`EncodeIntoViaEncode`].
pub trait EncodeInto: Send + Sync + Sized + Debug {
    /// Returns the exact number of bytes needed to encode this value.
    fn encoded_len(&self) -> usize;

    /// Encodes `self` into the provided buffer.
    ///
    /// # Panics
    /// May panic if `buf.len() < self.encoded_len()`.
    fn encode_into(&self, buf: &mut [u8]);
}

mod sealed {
    pub trait Sealed {}
}

/// Marker trait for types that should use the `Encode` + copy fallback for `EncodeInto`.
///
/// Implement this for types where cloning is acceptable or unavoidable.
/// For hot-path types with identity encoding (where in-memory bytes equal encoded bytes),
/// implement `EncodeInto` directly instead.
pub trait EncodeIntoViaEncode: sealed::Sealed + Send + Sync + Sized + Debug {}

/// Blanket impl for types that opt into the `Encode` + copy fallback.
impl<T, const N: usize> EncodeInto for T
where
    T: Encode<Encoded = [u8; N]> + EncodeIntoViaEncode + Copy,
{
    #[inline]
    fn encoded_len(&self) -> usize {
        N
    }

    #[inline]
    fn encode_into(&self, buf: &mut [u8]) {
        buf[..N].copy_from_slice((*self).encode().as_ref());
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

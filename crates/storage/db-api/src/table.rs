use crate::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
    transaction::{DbTx, DbTxMut},
    DatabaseError,
};
use bytes::BufMut;
use serde::{Deserialize, Serialize};
use std::{fmt::Debug, mem::MaybeUninit};

/// A [`BufMut`] that only counts bytes written without allocating.
///
/// Used to compute the size of a value before serializing it.
#[derive(Debug, Clone, Copy)]
pub struct CountingBuf {
    len: usize,
    /// Scratch buffer for `chunk_mut`.
    /// Only needs to be large enough for a single `put_u128` or similar primitive write.
    scratch: [MaybeUninit<u8>; 16],
}

impl Default for CountingBuf {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl CountingBuf {
    /// Creates a new `CountingBuf`.
    #[inline]
    pub const fn new() -> Self {
        Self { len: 0, scratch: [MaybeUninit::uninit(); 16] }
    }

    /// Returns the number of bytes written.
    #[inline]
    pub const fn len(&self) -> usize {
        self.len
    }

    /// Returns true if no bytes have been written.
    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }
}

unsafe impl BufMut for CountingBuf {
    #[inline]
    fn remaining_mut(&self) -> usize {
        usize::MAX - self.len
    }

    #[inline]
    unsafe fn advance_mut(&mut self, cnt: usize) {
        self.len += cnt;
    }

    #[inline]
    fn chunk_mut(&mut self) -> &mut bytes::buf::UninitSlice {
        bytes::buf::UninitSlice::uninit(&mut self.scratch)
    }

    #[inline]
    fn put_slice(&mut self, src: &[u8]) {
        self.len += src.len();
    }

    #[inline]
    fn put_u8(&mut self, _val: u8) {
        self.len += 1;
    }

    #[inline]
    fn put_bytes(&mut self, _val: u8, cnt: usize) {
        self.len += cnt;
    }
}

/// A [`BufMut`] that writes directly into a caller-provided `&mut [u8]`.
///
/// Used to serialize values directly into MDBX-managed memory.
/// This tracks the write position.
#[derive(Debug)]
pub struct SliceBuf<'a> {
    /// The full buffer.
    buf: &'a mut [u8],
    /// Current write position.
    pos: usize,
}

impl<'a> SliceBuf<'a> {
    /// Creates a new `SliceBuf` wrapping the given slice.
    #[inline]
    pub const fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    /// Returns the number of bytes written.
    #[inline]
    pub const fn written(&self) -> usize {
        self.pos
    }
}

unsafe impl BufMut for SliceBuf<'_> {
    #[inline]
    fn remaining_mut(&self) -> usize {
        self.buf.len() - self.pos
    }

    #[inline]
    unsafe fn advance_mut(&mut self, cnt: usize) {
        debug_assert!(self.pos + cnt <= self.buf.len());
        self.pos += cnt;
    }

    #[inline]
    fn chunk_mut(&mut self) -> &mut bytes::buf::UninitSlice {
        // SAFETY: We're returning a slice of uninitialized memory that BufMut can write to.
        unsafe {
            bytes::buf::UninitSlice::from_raw_parts_mut(
                self.buf.as_mut_ptr().add(self.pos),
                self.buf.len() - self.pos,
            )
        }
    }

    #[inline]
    fn put_slice(&mut self, src: &[u8]) {
        debug_assert!(self.remaining_mut() >= src.len());
        self.buf[self.pos..self.pos + src.len()].copy_from_slice(src);
        self.pos += src.len();
    }
}

/// Trait that will transform the data to be saved in the DB in a (ideally) compressed format
pub trait Compress: Send + Sync + Sized + Debug {
    /// Compressed type.
    type Compressed: bytes::BufMut + AsRef<[u8]> + Into<Vec<u8>> + Default + Send + Sync + Debug;

    /// If the type cannot be compressed, return its inner reference as `Some(self.as_ref())`
    fn uncompressable_ref(&self) -> Option<&[u8]> {
        None
    }

    /// Returns the exact number of bytes that [`Compress::compress_to_buf`] will write.
    ///
    /// This is used by MDBX reserve operations to allocate the exact buffer size needed.
    /// The default implementation uses a [`CountingBuf`] to compute the size without allocating.
    ///
    /// # Important
    ///
    /// This **must** return the exact same size that `compress_to_buf` will write.
    /// A mismatch will cause database corruption or panics.
    fn compressed_size(&self) -> usize {
        if let Some(uncompressable) = self.uncompressable_ref() {
            return uncompressable.len();
        }
        let mut buf = CountingBuf::new();
        self.compress_to_buf(&mut buf);
        buf.len()
    }

    /// Compresses data going into the database.
    fn compress(self) -> Self::Compressed {
        let mut buf = Self::Compressed::default();
        self.compress_to_buf(&mut buf);
        buf
    }

    /// Compresses data to a given buffer.
    fn compress_to_buf<B: bytes::BufMut>(&self, buf: &mut B);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counting_buf_tracks_bytes() {
        let mut buf = CountingBuf::new();
        assert_eq!(buf.len(), 0);
        assert!(buf.is_empty());

        buf.put_slice(&[1, 2, 3, 4, 5]);
        assert_eq!(buf.len(), 5);

        buf.put_u8(0xff);
        assert_eq!(buf.len(), 6);

        buf.put_bytes(0x00, 10);
        assert_eq!(buf.len(), 16);
        assert!(!buf.is_empty());
    }

    #[test]
    fn slice_buf_writes_correctly() {
        let mut backing = [0u8; 16];
        let mut buf = SliceBuf::new(&mut backing);

        assert_eq!(buf.written(), 0);
        assert_eq!(buf.remaining_mut(), 16);

        buf.put_slice(&[0x11, 0x22, 0x33]);
        assert_eq!(buf.written(), 3);
        assert_eq!(buf.remaining_mut(), 13);

        buf.put_u8(0x44);
        assert_eq!(buf.written(), 4);

        assert_eq!(&backing[..4], &[0x11, 0x22, 0x33, 0x44]);
    }

    #[test]
    fn compressed_size_matches_compress() {
        use alloy_primitives::{Address, B256};

        let addr = Address::repeat_byte(0x42);
        let addr_compressed = addr.compress();
        assert_eq!(addr.compressed_size(), addr_compressed.len());

        let hash = B256::repeat_byte(0xab);
        let hash_compressed = hash.compress();
        assert_eq!(hash.compressed_size(), hash_compressed.len());
    }

    #[test]
    fn uncompressable_types_fast_path() {
        use alloy_primitives::B256;

        let hash = B256::repeat_byte(0xcd);
        assert!(hash.uncompressable_ref().is_some());
        assert_eq!(hash.uncompressable_ref().unwrap().len(), 32);
    }
}

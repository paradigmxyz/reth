use crate::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
    transaction::{DbTx, DbTxMut},
    DatabaseError,
};
use bytes::BufMut;
use serde::{Deserialize, Serialize};
use std::fmt::Debug;

/// A [`BufMut`] that only counts bytes written without allocating.
///
/// Used to compute the size of a value before serializing it.
#[derive(Debug, Default, Clone, Copy)]
pub struct CountingBuf {
    len: usize,
}

impl CountingBuf {
    /// Thread-local scratch buffer for `chunk_mut` and `as_mut`.
    /// Must be large enough for any value that might call `buf.as_mut()[offset]` during counting.
    /// Transactions with large calldata can exceed 128KB, so we use 256KB to be safe.
    const SCRATCH_SIZE: usize = 256 * 1024;

    /// Creates a new `CountingBuf`.
    #[inline]
    pub const fn new() -> Self {
        Self { len: 0 }
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

std::thread_local! {
    /// Thread-local scratch buffer for [`CountingBuf::chunk_mut`].
    static SCRATCH: std::cell::UnsafeCell<[u8; CountingBuf::SCRATCH_SIZE]> =
        const { std::cell::UnsafeCell::new([0u8; CountingBuf::SCRATCH_SIZE]) };
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
        SCRATCH.with(|scratch| {
            // SAFETY: We return a mutable reference to the thread-local buffer.
            // This is safe because CountingBuf is not Sync and chunk_mut takes &mut self.
            unsafe {
                bytes::buf::UninitSlice::from_raw_parts_mut(
                    (*scratch.get()).as_mut_ptr(),
                    Self::SCRATCH_SIZE,
                )
            }
        })
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

impl AsMut<[u8]> for CountingBuf {
    #[inline]
    fn as_mut(&mut self) -> &mut [u8] {
        SCRATCH.with(|scratch| {
            // SAFETY: We return a mutable reference to the thread-local buffer.
            // This allows code that writes back to earlier positions (e.g., flags bytes)
            // to work during size counting. The writes are discarded.
            // We slice to self.len to match what Vec::as_mut() would return.
            debug_assert!(self.len <= Self::SCRATCH_SIZE, "CountingBuf overflow");
            unsafe { &mut (&mut (*scratch.get()))[..self.len] }
        })
    }
}

/// A [`BufMut`] that writes directly into a caller-provided `&mut [u8]`.
///
/// Used to serialize values directly into MDBX-managed memory.
/// Unlike a simple slice, this tracks the write position and provides
/// access to the written portion via `AsMut<[u8]>`.
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

impl AsMut<[u8]> for SliceBuf<'_> {
    #[inline]
    fn as_mut(&mut self) -> &mut [u8] {
        &mut self.buf[..self.pos]
    }
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

    /// Returns the exact number of bytes that [`Compress::compress_to_buf`] will write.
    ///
    /// This is used by [`mdbx::reserve`] to allocate the exact buffer size needed.
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

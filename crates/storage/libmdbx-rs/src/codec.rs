//! Codec for deserializing database values into Rust types.

use crate::{error::ReadResult, tx::ops, MdbxError, TransactionKind};
use ffi::MDBX_txn;
use std::{borrow::Cow, slice};

/// A marker trait for types that can be deserialized from a database value
/// without borrowing from the transaction.
///
/// Types implementing this trait can be used with iterators that need to
/// return owned values. This is automatically implemented for any type that
/// implements [`TableObject<'a>`] for all lifetimes `'a`.
///
/// # Built-in Implementations
///
/// - [`Vec<u8>`] - Always copies data
/// - `[u8; N]` - Copies into fixed-size array - may pan
/// - `()` - Ignores data entirely
/// - [`ObjectLength`] - Returns only the length
pub trait TableObjectOwned: for<'de> TableObject<'de> {
    /// Decodes the object from the given bytes, without borrowing them.
    ///
    /// [`ReadError`]: crate::ReadError
    fn decode(data_val: &[u8]) -> ReadResult<Self> {
        <Self as TableObject<'_>>::decode_borrow(Cow::Borrowed(data_val))
    }
}

impl<T> TableObjectOwned for T where T: for<'de> TableObject<'de> {}

/// Decodes values read from the database into Rust types.
///
/// Implement this trait to enable reading custom types directly from MDBX.
/// The lifetime parameter `'a` allows types to borrow data from the
/// transaction when appropriate (e.g., `Cow<'a, [u8]>`).
///
/// # Implementation Guide
///
/// For most types, only implement [`decode_borrow`](Self::decode_borrow). An
/// internal function `decode_val` is provided to handle zero-copy borrowing.
/// Implementing it is STRONGLY DISCOURAGED. Zero-copy borrowing can be
/// achieved by implementing `decode_borrow`.
///
/// ## Zero-copy Deserialization
///
/// MDBX supports zero-copy deserialization for types that can borrow data
/// directly from the database (like `Cow<'a, [u8]>`). Read-only transactions
/// ALWAYS support borrowing, while read-write transactions require a check
/// to see if the data is "dirty" (modified but not yet committed). If the page
/// containing the data is dirty, a copy must be made before borrowing.
///
/// [`TableObject::decode_borrow`] is the main method to implement. It receives
/// a `Cow<'a, [u8]>` which may be either borrowed or owned data, depending on
/// the transaction type and data state. Implementations may choose to split
/// the data further, copy it, or use it as-is.
///
/// ```
/// # use std::borrow::Cow;
/// use reth_libmdbx::{MdbxError, ReadResult, TableObject};
///
/// // A zero-copy wrapper around a u32 stored in little-endian format.
/// struct MyZeroCopyU32<'a>(Cow<'a, [u8]>);
///
/// impl<'a> TableObject<'a> for MyZeroCopyU32<'a> {
///     fn decode_borrow(data: Cow<'a, [u8]>) -> ReadResult<Self> {
///         if data.len() < 4 {
///             return Err(MdbxError::DecodeErrorLenDiff.into());
///         }
///         Ok(MyZeroCopyU32(data))
///     }
/// }
///
/// impl MyZeroCopyU32<'_> {
///     /// Reads a u32 from the start of the data.
///     pub fn read_u32(&self) -> u32 {
///         let bytes = &self.0[..4];
///         u32::from_le_bytes(bytes.try_into().unwrap())
///     }
/// }
/// ```
///
/// ## Fixed-Size Types
///
/// ```
/// # use std::borrow::Cow;
/// # use reth_libmdbx::{TableObject, ReadResult, MdbxError};
/// struct Hash([u8; 32]);
///
/// impl TableObject<'_> for Hash {
///     fn decode_borrow(data: Cow<'_, [u8]>) -> ReadResult<Self> {
///         let arr: [u8; 32] =
///             data.as_ref().try_into().map_err(|_| MdbxError::DecodeErrorLenDiff)?;
///         Ok(Self(arr))
///     }
/// }
/// ```
///
/// ## Variable-Size Types
///
/// ```
/// # use std::borrow::Cow;
/// # use reth_libmdbx::{TableObject, ReadResult, MdbxError};
/// struct VarInt(u64);
///
/// impl TableObject<'_> for VarInt {
///     fn decode_borrow(data: Cow<'_, [u8]>) -> ReadResult<Self> {
///         // Example: decode LEB128 or similar
///         let value = data
///             .iter()
///             .take(8)
///             .enumerate()
///             .fold(0u64, |acc, (i, &b)| acc | ((b as u64) << (i * 8)));
///         Ok(Self(value))
///     }
/// }
/// ```
pub trait TableObject<'a>: Sized {
    /// Creates the object from a `Cow` of bytes. This allows for efficient
    /// handling of both owned and borrowed data.
    fn decode_borrow(data: Cow<'a, [u8]>) -> ReadResult<Self>;

    /// Decodes the value directly from the given MDBX_val pointer.
    ///
    /// **Do not implement this unless you need zero-copy borrowing and cannot
    /// handle the [`Cow`] overhead**.
    ///
    /// This method is used internally to optimize deserialization for types
    /// that borrow data directly from the database (like `Cow<'a, [u8]>`).
    ///
    /// # Safety
    ///
    /// The data pointed to by `data_val` is only valid for the lifetime of
    /// the transaction. In read-write transactions, the data may be "dirty"
    /// (modified but not yet committed), requiring a copy via `mdbx_is_dirty`
    /// before borrowing.
    ///
    /// The caller must ensure that `tx` is a valid pointer to the current
    /// transaction AND that `data_val` points to valid MDBX data AND that
    /// they have exclusive access to the transaction if it is read-write.
    #[doc(hidden)]
    #[inline(always)]
    unsafe fn decode_val<K: TransactionKind>(
        tx: *const MDBX_txn,
        data_val: ffi::MDBX_val,
    ) -> ReadResult<Self> {
        let cow = unsafe { Cow::<'a, [u8]>::decode_val::<K>(tx, data_val)? };
        Self::decode_borrow(cow)
    }
}

impl<'a> TableObject<'a> for Cow<'a, [u8]> {
    fn decode_borrow(data: Cow<'a, [u8]>) -> ReadResult<Self> {
        Ok(data)
    }

    #[doc(hidden)]
    unsafe fn decode_val<K: TransactionKind>(
        txn: *const MDBX_txn,
        data_val: ffi::MDBX_val,
    ) -> ReadResult<Self> {
        // SAFETY: Caller ensures the tx is active, slice is valid for lifetime
        // 'a.
        let s = unsafe { slice::from_raw_parts(data_val.iov_base as *const u8, data_val.iov_len) };

        // SAFETY: txn is valid from caller, data_val.iov_base points to db pages.
        let is_dirty = (!K::IS_READ_ONLY) && unsafe { ops::is_dirty_raw(txn, data_val.iov_base) }?;

        Ok(if is_dirty { Cow::Owned(s.to_vec()) } else { Cow::Borrowed(s) })
    }
}

impl TableObject<'_> for Vec<u8> {
    fn decode_borrow(data: Cow<'_, [u8]>) -> ReadResult<Self> {
        Ok(data.into_owned())
    }

    unsafe fn decode_val<K: TransactionKind>(
        _tx: *const MDBX_txn,
        data_val: ffi::MDBX_val,
    ) -> ReadResult<Self> {
        // SAFETY: Caller ensures the tx is active, slice is valid for lifetime.
        // We always copy for Vec<u8> since we need to own the data.
        let s = unsafe { slice::from_raw_parts(data_val.iov_base as *const u8, data_val.iov_len) };
        Ok(s.to_vec())
    }
}

impl<'a> TableObject<'a> for () {
    fn decode_borrow(_: Cow<'a, [u8]>) -> ReadResult<Self> {
        Ok(())
    }

    unsafe fn decode_val<K: TransactionKind>(
        _: *const MDBX_txn,
        _: ffi::MDBX_val,
    ) -> ReadResult<Self> {
        Ok(())
    }
}

/// If you don't need the data itself, just its length.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(transparent)]
pub struct ObjectLength(pub usize);

impl TableObject<'_> for ObjectLength {
    fn decode_borrow(data: Cow<'_, [u8]>) -> ReadResult<Self> {
        Ok(Self(data.len()))
    }

    unsafe fn decode_val<K: TransactionKind>(
        _tx: *const MDBX_txn,
        data_val: ffi::MDBX_val,
    ) -> ReadResult<Self> {
        Ok(Self(data_val.iov_len))
    }
}

impl core::ops::Deref for ObjectLength {
    type Target = usize;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'a, const LEN: usize> TableObject<'a> for [u8; LEN] {
    fn decode_borrow(data: Cow<'a, [u8]>) -> ReadResult<Self> {
        if data.len() != LEN {
            return Err(MdbxError::DecodeErrorLenDiff.into());
        }
        let mut a = [0; LEN];
        a[..].copy_from_slice(&data);
        Ok(a)
    }

    unsafe fn decode_val<K: TransactionKind>(
        _tx: *const MDBX_txn,
        data_val: ffi::MDBX_val,
    ) -> ReadResult<Self> {
        // SAFETY: Caller ensures the tx is active, slice is valid.
        if data_val.iov_len != LEN {
            return Err(MdbxError::DecodeErrorLenDiff.into());
        }
        let s = unsafe { slice::from_raw_parts(data_val.iov_base as *const u8, data_val.iov_len) };
        let mut a = [0; LEN];
        a[..].copy_from_slice(s);
        Ok(a)
    }
}

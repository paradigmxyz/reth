use crate::{Error, TransactionKind};
use derive_more::{Debug, Deref, DerefMut};
use std::{borrow::Cow, slice};

/// A marker trait for types that can be deserialized from a database value
/// without borrowing from the transaction.
///
/// Types implementing this trait can be used with iterators that need to
/// return owned values. This is automatically implemented for any type that
/// implements [`TableObject<'a>`] for all lifetimes `'a`.
pub trait TableObjectOwned: for<'de> TableObject<'de> {
    /// Decodes the object from the given bytes, without borrowing them.
    fn decode(data_val: &[u8]) -> Result<Self, Error> {
        <Self as TableObject<'_>>::decode_borrow(Cow::Borrowed(data_val))
    }
}

impl<T> TableObjectOwned for T where T: for<'de> TableObject<'de> {}

/// Decodes values read from the database into Rust types.
///
/// Implement this to be able to decode data values. The lifetime parameter `'a`
/// allows types to borrow data from the transaction when appropriate (e.g.,
/// `Cow<'a, [u8]>`).
pub trait TableObject<'a>: Sized {
    /// Creates the object from a `Cow` of bytes. This allows for efficient
    /// handling of both owned and borrowed data.
    fn decode_borrow(data: Cow<'a, [u8]>) -> Result<Self, Error>;

    /// Decodes the value directly from the given MDBX_val pointer.
    ///
    /// # Safety
    ///
    /// This should only be called in the context of an MDBX transaction.
    #[doc(hidden)]
    unsafe fn decode_val<K: TransactionKind>(
        tx: *const ffi::MDBX_txn,
        data_val: ffi::MDBX_val,
    ) -> Result<Self, Error> {
        let cow = unsafe { Cow::<'a, [u8]>::decode_val::<K>(tx, data_val)? };
        Self::decode_borrow(cow)
    }
}

impl<'a> TableObject<'a> for Cow<'a, [u8]> {
    fn decode_borrow(data: Cow<'a, [u8]>) -> Result<Self, Error> {
        Ok(data)
    }

    #[doc(hidden)]
    unsafe fn decode_val<K: TransactionKind>(
        _txn: *const ffi::MDBX_txn,
        data_val: ffi::MDBX_val,
    ) -> Result<Self, Error> {
        let s = unsafe { slice::from_raw_parts(data_val.iov_base as *const u8, data_val.iov_len) };

        #[cfg(feature = "return-borrowed")]
        {
            Ok(Cow::Borrowed(s))
        }

        #[cfg(not(feature = "return-borrowed"))]
        {
            let is_dirty = (!K::IS_READ_ONLY) &&
                crate::error::mdbx_result(unsafe {
                    ffi::mdbx_is_dirty(_txn, data_val.iov_base)
                })?;

            Ok(if is_dirty { Cow::Owned(s.to_vec()) } else { Cow::Borrowed(s) })
        }
    }
}

impl TableObject<'_> for Vec<u8> {
    fn decode_borrow(data: Cow<'_, [u8]>) -> Result<Self, Error> {
        Ok(data.into_owned())
    }

    unsafe fn decode_val<K: TransactionKind>(
        _tx: *const ffi::MDBX_txn,
        data_val: ffi::MDBX_val,
    ) -> Result<Self, Error> {
        let s = unsafe { slice::from_raw_parts(data_val.iov_base as *const u8, data_val.iov_len) };
        Ok(s.to_vec())
    }
}

impl TableObject<'_> for () {
    fn decode_borrow(_: Cow<'_, [u8]>) -> Result<Self, Error> {
        Ok(())
    }

    unsafe fn decode_val<K: TransactionKind>(
        _: *const ffi::MDBX_txn,
        _: ffi::MDBX_val,
    ) -> Result<Self, Error> {
        Ok(())
    }
}

/// If you don't need the data itself, just its length.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Deref, DerefMut)]
pub struct ObjectLength(pub usize);

impl TableObject<'_> for ObjectLength {
    fn decode_borrow(data: Cow<'_, [u8]>) -> Result<Self, Error> {
        Ok(Self(data.len()))
    }

    unsafe fn decode_val<K: TransactionKind>(
        _tx: *const ffi::MDBX_txn,
        data_val: ffi::MDBX_val,
    ) -> Result<Self, Error> {
        Ok(Self(data_val.iov_len))
    }
}

impl<const LEN: usize> TableObject<'_> for [u8; LEN] {
    fn decode_borrow(data: Cow<'_, [u8]>) -> Result<Self, Error> {
        if data.len() != LEN {
            return Err(Error::DecodeErrorLenDiff);
        }
        let mut a = [0; LEN];
        a[..].copy_from_slice(&data);
        Ok(a)
    }
}

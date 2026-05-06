//! Small database table utilities and helper functions.

use crate::{
    table::{Decode, Decompress, Table, TableRow},
    DatabaseError,
};
use std::borrow::Cow;

#[cfg(unix)]
const BYTECODE_READAHEAD_MIN_LEN: usize = 8 * 1024;

/// Helper function to decode a `(key, value)` pair.
pub(crate) fn decoder<'a, T>(
    (k, v): (Cow<'a, [u8]>, Cow<'a, [u8]>),
) -> Result<TableRow<T>, DatabaseError>
where
    T: Table,
    T::Key: Decode,
    T::Value: Decompress,
{
    Ok((
        match k {
            Cow::Borrowed(k) => Decode::decode(k)?,
            Cow::Owned(k) => Decode::decode_owned(k)?,
        },
        decompress_table_value::<T>(v)?,
    ))
}

/// Helper function to decode only a value from a `(key, value)` pair.
pub(crate) fn decode_value<'a, T>(
    kv: (Cow<'a, [u8]>, Cow<'a, [u8]>),
) -> Result<T::Value, DatabaseError>
where
    T: Table,
{
    decompress_table_value::<T>(kv.1)
}

/// Helper function to decode a value. It can be a key or subkey.
pub(crate) fn decode_one<T>(value: Cow<'_, [u8]>) -> Result<T::Value, DatabaseError>
where
    T: Table,
{
    decompress_table_value::<T>(value)
}

fn decompress_table_value<T>(value: Cow<'_, [u8]>) -> Result<T::Value, DatabaseError>
where
    T: Table,
{
    Ok(match value {
        Cow::Borrowed(v) => {
            maybe_madvise_bytecode_value::<T>(v);
            Decompress::decompress(v)?
        }
        Cow::Owned(v) => Decompress::decompress_owned(v)?,
    })
}

#[inline]
fn maybe_madvise_bytecode_value<T: Table>(value: &[u8]) {
    if T::NAME == <crate::tables::Bytecodes as Table>::NAME {
        madvise_willneed(value);
    }
}

#[cfg(unix)]
fn madvise_willneed(value: &[u8]) {
    if value.len() < BYTECODE_READAHEAD_MIN_LEN {
        return;
    }

    let page_size = page_size::get();
    let start = value.as_ptr() as usize;
    let page_start = start / page_size * page_size;
    let end = start.saturating_add(value.len());
    let page_end = end.div_ceil(page_size) * page_size;

    // MDBX disables broad readahead because most access is random. Large bytecode
    // values are different: decoding immediately scans the whole mmap-backed value.
    let _ = unsafe {
        libc::madvise(page_start as *mut libc::c_void, page_end - page_start, libc::MADV_WILLNEED)
    };
}

#[cfg(not(unix))]
#[inline]
const fn madvise_willneed(_value: &[u8]) {}

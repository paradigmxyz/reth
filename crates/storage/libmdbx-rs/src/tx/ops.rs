//! Shared low-level FFI operations for transactions.
//!
//! This module contains `#[inline(always)]` functions that wrap raw MDBX FFI
//! calls. These functions take raw `*mut ffi::MDBX_txn` pointers and are
//! designed to be called from [`TxAccess::with_txn_ptr`].
//!
//! # Safety
//!
//!  - All functions require the caller to ensure the provided transaction pointer is BOTH **Valid**
//!    and **Exclusive**.
//!
//! MDBX's transaction model requires that a transaction pointer is only used
//! by a single thread at a time. Therefore, callers must ensure that no
//! other code is concurrently accessing the same transaction pointer while
//! these functions are being called.
//!
//! Failure to uphold these safety guarantees may lead to undefined behavior,
//! data corruption, crashes, or other serious issues.
//!
//! [`TxAccess::with_txn_ptr`]: crate::tx::access::TxPtrAccess::with_txn_ptr

use crate::{
    error::{mdbx_result, MdbxResult},
    flags::{DatabaseFlags, WriteFlags},
    Stat,
};
use std::{
    ffi::{c_char, c_uint, c_void},
    mem::size_of,
    ptr, slice,
};

/// Performs a raw `mdbx_get` operation.
///
/// Returns the data value if found, `None` if not found.
///
/// # Safety
///
/// - `txn` must be a valid, non-null transaction pointer.
/// - `dbi` must be a valid database handle for this transaction.
/// - `key` must be a valid slice.
#[inline(always)]
pub(crate) unsafe fn get_raw(
    txn: *mut ffi::MDBX_txn,
    dbi: ffi::MDBX_dbi,
    key: &[u8],
) -> MdbxResult<Option<ffi::MDBX_val>> {
    let key_val = ffi::MDBX_val { iov_len: key.len(), iov_base: key.as_ptr() as *mut c_void };
    let mut data_val = ffi::MDBX_val { iov_len: 0, iov_base: ptr::null_mut() };

    // SAFETY: Caller guarantees txn and dbi are valid.
    match unsafe { ffi::mdbx_get(txn, dbi, &key_val, &mut data_val) } {
        ffi::MDBX_SUCCESS => Ok(Some(data_val)),
        ffi::MDBX_NOTFOUND => Ok(None),
        err_code => Err(crate::MdbxError::from_err_code(err_code)),
    }
}

/// Opens a database and retrieves its flags.
///
/// Returns `(dbi, flags)` on success.
///
/// # Safety
///
/// - `txn` must be a valid, non-null transaction pointer.
/// - `name_ptr` must be a valid C string pointer or null for the default DB.
#[inline(always)]
pub(crate) unsafe fn open_db_raw(
    txn: *mut ffi::MDBX_txn,
    name_ptr: *const c_char,
    flags: DatabaseFlags,
) -> MdbxResult<(ffi::MDBX_dbi, DatabaseFlags)> {
    let mut dbi: ffi::MDBX_dbi = 0;
    let mut actual_flags: c_uint = 0;
    let mut _status: c_uint = 0;

    // SAFETY: Caller guarantees txn is valid, name_ptr is valid or null.
    mdbx_result(unsafe { ffi::mdbx_dbi_open(txn, name_ptr, flags.bits(), &mut dbi) })?;
    // SAFETY: Caller guarantees txn is valid, dbi was just opened.
    mdbx_result(unsafe { ffi::mdbx_dbi_flags_ex(txn, dbi, &mut actual_flags, &mut _status) })?;

    #[cfg_attr(not(windows), allow(clippy::useless_conversion))]
    let db_flags = DatabaseFlags::from_bits_truncate(actual_flags.try_into().unwrap());

    Ok((dbi, db_flags))
}

/// Retrieves the flags for an open database.
///
/// # Safety
///
/// - `txn` must be a valid, non-null transaction pointer.
/// - `dbi` must be a valid database handle for this transaction.
#[inline(always)]
pub(crate) unsafe fn db_flags_raw(
    txn: *mut ffi::MDBX_txn,
    dbi: ffi::MDBX_dbi,
) -> MdbxResult<DatabaseFlags> {
    let mut flags: c_uint = 0;
    let mut _status: c_uint = 0;

    // SAFETY: Caller guarantees txn and dbi are valid.
    mdbx_result(unsafe { ffi::mdbx_dbi_flags_ex(txn, dbi, &mut flags, &mut _status) })?;

    #[cfg_attr(not(windows), allow(clippy::useless_conversion))]
    Ok(DatabaseFlags::from_bits_truncate(flags.try_into().unwrap()))
}

/// Retrieves database statistics.
///
/// # Safety
///
/// - `txn` must be a valid, non-null transaction pointer.
/// - `dbi` must be a valid database handle for this transaction.
#[inline(always)]
pub(crate) unsafe fn db_stat_raw(txn: *mut ffi::MDBX_txn, dbi: ffi::MDBX_dbi) -> MdbxResult<Stat> {
    let mut stat = Stat::new();
    // SAFETY: Caller guarantees txn and dbi are valid.
    mdbx_result(unsafe { ffi::mdbx_dbi_stat(txn, dbi, stat.mdb_stat(), size_of::<Stat>()) })?;
    Ok(stat)
}

/// Commits a transaction.
///
/// Returns `true` if the transaction was aborted (botched), `false` otherwise.
///
/// # Safety
///
/// - `txn` must be a valid, non-null transaction pointer.
/// - `latency` may be null or a valid pointer to receive latency info.
#[inline(always)]
pub(crate) unsafe fn commit_raw(
    txn: *mut ffi::MDBX_txn,
    latency: *mut ffi::MDBX_commit_latency,
) -> MdbxResult<bool> {
    // SAFETY: Caller guarantees txn is valid, latency is null or valid.
    mdbx_result(unsafe { ffi::mdbx_txn_commit_ex(txn, latency) })
}

/// Stores a key/value pair in the database.
///
/// # Safety
///
/// - `txn` must be a valid, non-null RW transaction pointer.
/// - `dbi` must be a valid database handle for this transaction.
#[inline(always)]
pub(crate) unsafe fn put_raw(
    txn: *mut ffi::MDBX_txn,
    dbi: ffi::MDBX_dbi,
    key: &[u8],
    data: &[u8],
    flags: WriteFlags,
) -> MdbxResult<()> {
    let key_val = ffi::MDBX_val { iov_len: key.len(), iov_base: key.as_ptr() as *mut c_void };
    let mut data_val =
        ffi::MDBX_val { iov_len: data.len(), iov_base: data.as_ptr() as *mut c_void };

    // SAFETY: Caller guarantees txn and dbi are valid.
    mdbx_result(unsafe { ffi::mdbx_put(txn, dbi, &key_val, &mut data_val, flags.bits()) })?;
    Ok(())
}

/// Reserves space for a value and returns a mutable slice to write into.
///
/// # Safety
///
/// - `txn` must be a valid, non-null RW transaction pointer.
/// - `dbi` must be a valid database handle for this transaction.
/// - The returned slice is only valid until the transaction is committed, aborted, or another value
///   is inserted at the same key.
#[inline(always)]
pub(crate) unsafe fn reserve_raw(
    txn: *mut ffi::MDBX_txn,
    dbi: ffi::MDBX_dbi,
    key: &[u8],
    len: usize,
    flags: WriteFlags,
) -> MdbxResult<*mut u8> {
    let key_val = ffi::MDBX_val { iov_len: key.len(), iov_base: key.as_ptr() as *mut c_void };
    let mut data_val = ffi::MDBX_val { iov_len: len, iov_base: ptr::null_mut::<c_void>() };

    // SAFETY: Caller guarantees txn and dbi are valid.
    mdbx_result(unsafe {
        ffi::mdbx_put(txn, dbi, &key_val, &mut data_val, flags.bits() | ffi::MDBX_RESERVE)
    })?;

    Ok(data_val.iov_base as *mut u8)
}

/// Creates a slice from a reserved pointer.
///
/// # Safety
///
/// - `ptr` must be a valid pointer returned from `reserve_raw`.
/// - `len` must match the length passed to `reserve_raw`.
#[inline(always)]
pub(crate) const unsafe fn slice_from_reserved(ptr: *mut u8, len: usize) -> &'static mut [u8] {
    // SAFETY: Caller guarantees ptr is valid and len matches.
    unsafe { slice::from_raw_parts_mut(ptr, len) }
}

/// Deletes items from a database.
///
/// Returns `true` if the key/value pair was present, `false` if not found.
///
/// # Safety
///
/// - `txn` must be a valid, non-null RW transaction pointer.
/// - `dbi` must be a valid database handle for this transaction.
#[inline(always)]
pub(crate) unsafe fn del_raw(
    txn: *mut ffi::MDBX_txn,
    dbi: ffi::MDBX_dbi,
    key: &[u8],
    data: Option<&[u8]>,
) -> MdbxResult<bool> {
    let key_val = ffi::MDBX_val { iov_len: key.len(), iov_base: key.as_ptr() as *mut c_void };
    let data_val: Option<ffi::MDBX_val> = data
        .map(|data| ffi::MDBX_val { iov_len: data.len(), iov_base: data.as_ptr() as *mut c_void });

    let ptr = data_val.as_ref().map_or(ptr::null(), |d| d as *const ffi::MDBX_val);

    // SAFETY: Caller guarantees txn and dbi are valid.
    mdbx_result(unsafe { ffi::mdbx_del(txn, dbi, &key_val, ptr) }).map(|_| true).or_else(
        |e| match e {
            crate::MdbxError::NotFound => Ok(false),
            other => Err(other),
        },
    )
}

/// Empties a database (removes all items).
///
/// # Safety
///
/// - `txn` must be a valid, non-null RW transaction pointer.
/// - `dbi` must be a valid database handle for this transaction.
#[inline(always)]
pub(crate) unsafe fn clear_db_raw(txn: *mut ffi::MDBX_txn, dbi: ffi::MDBX_dbi) -> MdbxResult<()> {
    // SAFETY: Caller guarantees txn and dbi are valid.
    mdbx_result(unsafe { ffi::mdbx_drop(txn, dbi, false) })?;
    Ok(())
}

/// Drops a database from the environment.
///
/// # Safety
///
/// - `txn` must be a valid, non-null RW transaction pointer.
/// - `dbi` must be a valid database handle for this transaction.
/// - Caller must ensure all other Database and Cursor instances pointing to this dbi are closed
///   before calling.
#[inline(always)]
pub(crate) unsafe fn drop_db_raw(txn: *mut ffi::MDBX_txn, dbi: ffi::MDBX_dbi) -> MdbxResult<()> {
    // SAFETY: Caller guarantees txn and dbi are valid.
    mdbx_result(unsafe { ffi::mdbx_drop(txn, dbi, true) })?;
    Ok(())
}

/// Closes a database handle.
///
/// # Safety
///
/// - `env` must be a valid, non-null environment pointer.
/// - `dbi` must be a valid database handle for this environment.
/// - Caller must ensure the database is not in use.
#[inline(always)]
pub(crate) unsafe fn close_db_raw(env: *mut ffi::MDBX_env, dbi: ffi::MDBX_dbi) -> MdbxResult<()> {
    // SAFETY: Caller guarantees env and dbi are valid.
    mdbx_result(unsafe { ffi::mdbx_dbi_close(env, dbi) })?;
    Ok(())
}

/// Checks if a memory pointer refers to dirty (modified) data.
///
/// Returns `true` if the data is dirty and must be copied before borrowing.
///
/// # Safety
///
/// - `txn` must be a valid, non-null transaction pointer.
/// - `ptr` must be a pointer to data within the transaction's database pages.
/// - Access MUST be serialized. Concurrent access is NOT ALLOWED.
#[inline(always)]
pub(crate) unsafe fn is_dirty_raw(
    txn: *const ffi::MDBX_txn,
    ptr: *const c_void,
) -> MdbxResult<bool> {
    // SAFETY: Caller guarantees txn and ptr are valid.
    mdbx_result(unsafe { ffi::mdbx_is_dirty(txn, ptr) })
}

// =============================================================================
// Debug-only helpers for append assertions
// =============================================================================

/// Gets pagesize from transaction environment.
///
/// # Safety
///
/// - `txn` must be a valid, non-null transaction pointer.
#[cfg(debug_assertions)]
unsafe fn get_pagesize(txn: *mut ffi::MDBX_txn) -> usize {
    // SAFETY: Caller guarantees txn is valid.
    let env_ptr = unsafe { ffi::mdbx_txn_env(txn) };
    let mut stat: ffi::MDBX_stat = unsafe { std::mem::zeroed() };
    // SAFETY: env_ptr is valid from mdbx_txn_env.
    unsafe { ffi::mdbx_env_stat_ex(env_ptr, ptr::null(), &mut stat, size_of::<ffi::MDBX_stat>()) };
    stat.ms_psize as usize
}

/// Gets last key from database using a temporary cursor.
///
/// # Safety
///
/// - `txn` must be a valid, non-null transaction pointer.
/// - `dbi` must be a valid database handle for this transaction.
#[cfg(debug_assertions)]
unsafe fn get_last_key(txn: *mut ffi::MDBX_txn, dbi: ffi::MDBX_dbi) -> Option<Vec<u8>> {
    let mut cursor: *mut ffi::MDBX_cursor = ptr::null_mut();
    // SAFETY: Caller guarantees txn and dbi are valid.
    if unsafe { ffi::mdbx_cursor_open(txn, dbi, &mut cursor) } != ffi::MDBX_SUCCESS {
        return None;
    }
    let mut key_val = ffi::MDBX_val { iov_len: 0, iov_base: ptr::null_mut() };
    let mut data_val = ffi::MDBX_val { iov_len: 0, iov_base: ptr::null_mut() };
    // SAFETY: cursor was just opened successfully.
    let result =
        if unsafe { ffi::mdbx_cursor_get(cursor, &mut key_val, &mut data_val, ffi::MDBX_LAST) } ==
            ffi::MDBX_SUCCESS
        {
            // SAFETY: mdbx_cursor_get returned success, so key_val is valid.
            Some(unsafe {
                slice::from_raw_parts(key_val.iov_base as *const u8, key_val.iov_len).to_vec()
            })
        } else {
            None
        };
    // SAFETY: cursor is valid.
    unsafe { ffi::mdbx_cursor_close(cursor) };
    result
}

/// Gets last dup for a key using a temporary cursor.
///
/// # Safety
///
/// - `txn` must be a valid, non-null transaction pointer.
/// - `dbi` must be a valid database handle for this transaction.
#[cfg(debug_assertions)]
unsafe fn get_last_dup(txn: *mut ffi::MDBX_txn, dbi: ffi::MDBX_dbi, key: &[u8]) -> Option<Vec<u8>> {
    let mut cursor: *mut ffi::MDBX_cursor = ptr::null_mut();
    // SAFETY: Caller guarantees txn and dbi are valid.
    if unsafe { ffi::mdbx_cursor_open(txn, dbi, &mut cursor) } != ffi::MDBX_SUCCESS {
        return None;
    }
    let mut key_val = ffi::MDBX_val { iov_len: key.len(), iov_base: key.as_ptr() as *mut c_void };
    let mut data_val = ffi::MDBX_val { iov_len: 0, iov_base: ptr::null_mut() };

    // SAFETY: cursor was just opened successfully.
    let result =
        if unsafe { ffi::mdbx_cursor_get(cursor, &mut key_val, &mut data_val, ffi::MDBX_SET) } ==
            ffi::MDBX_SUCCESS
        {
            // SAFETY: cursor is positioned at key.
            if unsafe {
                ffi::mdbx_cursor_get(cursor, &mut key_val, &mut data_val, ffi::MDBX_LAST_DUP)
            } == ffi::MDBX_SUCCESS
            {
                // SAFETY: mdbx_cursor_get returned success, so data_val is valid.
                Some(unsafe {
                    slice::from_raw_parts(data_val.iov_base as *const u8, data_val.iov_len).to_vec()
                })
            } else {
                None
            }
        } else {
            None
        };
    // SAFETY: cursor is valid.
    unsafe { ffi::mdbx_cursor_close(cursor) };
    result
}

/// All-in-one append assertion: opens cursor, gets last key, asserts, closes cursor.
///
/// # Safety
///
/// - `txn` must be a valid, non-null transaction pointer.
/// - `dbi` must be a valid database handle for this transaction.
#[cfg(debug_assertions)]
pub(crate) unsafe fn debug_assert_append(
    txn: *mut ffi::MDBX_txn,
    dbi: ffi::MDBX_dbi,
    flags: DatabaseFlags,
    key: &[u8],
    data: &[u8],
) {
    // SAFETY: Caller guarantees txn is valid.
    let pagesize = unsafe { get_pagesize(txn) };
    // SAFETY: Caller guarantees txn and dbi are valid.
    let last_key = unsafe { get_last_key(txn, dbi) };
    super::assertions::debug_assert_append(pagesize, flags, key, data, last_key.as_deref());
}

/// All-in-one append_dup assertion: opens cursor, gets last dup, asserts, closes cursor.
///
/// # Safety
///
/// - `txn` must be a valid, non-null transaction pointer.
/// - `dbi` must be a valid database handle for this transaction.
#[cfg(debug_assertions)]
pub(crate) unsafe fn debug_assert_append_dup(
    txn: *mut ffi::MDBX_txn,
    dbi: ffi::MDBX_dbi,
    flags: DatabaseFlags,
    key: &[u8],
    data: &[u8],
) {
    // SAFETY: Caller guarantees txn is valid.
    let pagesize = unsafe { get_pagesize(txn) };
    // SAFETY: Caller guarantees txn and dbi are valid.
    let last_dup = unsafe { get_last_dup(txn, dbi, key) };
    super::assertions::debug_assert_append_dup(pagesize, flags, key, data, last_dup.as_deref());
}

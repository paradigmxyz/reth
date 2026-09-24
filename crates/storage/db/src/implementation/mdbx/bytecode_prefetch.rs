//! Targeted read-ahead for bytecode values spanning multiple OS pages.

use reth_libmdbx::{ffi, TableObject, TransactionKind};
use std::{borrow::Cow, sync::LazyLock};

/// Prefetches mapped bytecode before it is copied or decompressed.
pub(super) struct PrefetchBytecode<'a>(pub(super) Cow<'a, [u8]>);

impl<'a> TableObject for PrefetchBytecode<'a> {
    fn decode(_: &[u8]) -> reth_libmdbx::Result<Self> {
        unreachable!("bytecode prefetch requires the MDBX transaction context")
    }

    unsafe fn decode_val<K: TransactionKind>(
        txn: *const ffi::MDBX_txn,
        value: ffi::MDBX_val,
    ) -> reth_libmdbx::Result<Self> {
        // SAFETY: MDBX invokes this decoder with a value from the live transaction.
        if let Some((start, len)) = unsafe { prefetch_range::<K>(txn, value) } {
            // MDBX disables general read-ahead. Hint only this value's pages to avoid
            // serial faults while decoding large bytecode, without enabling general read-ahead.
            // SAFETY: The complete rounded range belongs to the live MDBX file mapping.
            // Advice is best-effort and must not turn a successful lookup into an error.
            let _ = unsafe { libc::madvise(start as *mut libc::c_void, len, libc::MADV_WILLNEED) };
        }
        // SAFETY: Preserve libmdbx's borrowing/copying behavior for the original value.
        unsafe { <Cow<'a, [u8]> as TableObject>::decode_val::<K>(txn, value).map(Self) }
    }
}

static PAGE_SIZE: LazyLock<usize> = LazyLock::new(page_size::get);

/// Returns the OS pages to prefetch, excluding small and dirty values.
///
/// # Safety
/// `value` must be the unmodified value returned by MDBX for the live `txn` of kind `K`.
unsafe fn prefetch_range<K: TransactionKind>(
    txn: *const ffi::MDBX_txn,
    value: ffi::MDBX_val,
) -> Option<(usize, usize)> {
    if value.iov_len <= *PAGE_SIZE {
        return None;
    }
    // Pipeline execution also reads unchanged bytecode through writable transactions.
    // Dirty values can be heap-backed; never advise them, even with `return-borrowed`.
    // SAFETY: This is the original value pointer, before any copy or transaction mutation.
    if !K::IS_READ_ONLY && unsafe { ffi::mdbx_is_dirty(txn, value.iov_base) } != ffi::MDBX_SUCCESS {
        return None;
    }
    page_range(value.iov_base as usize, value.iov_len, *PAGE_SIZE)
}

/// Rounds a nonempty value's range to complete OS pages, checking for overflow.
fn page_range(address: usize, len: usize, page_size: usize) -> Option<(usize, usize)> {
    if len == 0 || !page_size.is_power_of_two() {
        return None;
    }
    let start = address & !(page_size - 1);
    let end = address.checked_add(len)?.checked_add(page_size - 1)? & !(page_size - 1);
    Some((start, end.checked_sub(start)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{mdbx::DatabaseArguments, tables, DatabaseEnv, DatabaseEnvKind};
    use alloy_primitives::B256;
    use reth_db_api::{
        database::Database,
        models::ClientVersion,
        table::Encode,
        transaction::{DbTx, DbTxMut},
    };
    use reth_primitives_traits::Bytecode;

    #[test]
    fn rounds_to_os_pages() {
        assert_eq!(page_range(0x1014, 27_693, 4096), Some((0x1000, 7 * 4096)));
        assert_eq!(page_range(0x1000, 4096, 4096), Some((0x1000, 4096)));
        assert_eq!(page_range(0x1fff, 2, 4096), Some((0x1000, 8192)));
        assert_eq!(page_range(0x1000, 0, 4096), None);
        assert_eq!(page_range(usize::MAX - 10, 20, 4096), None);
        assert_eq!(page_range(usize::MAX - 10, 1, 4096), None);
        assert_eq!(page_range(0x1000, 4096, 0), None);
        assert_eq!(page_range(0x1000, 4096, 3), None);
    }

    /// Inspects the same prefetch eligibility decision used by the bytecode decoder.
    struct PrefetchRange(Option<(usize, usize)>);

    impl TableObject for PrefetchRange {
        fn decode(_: &[u8]) -> reth_libmdbx::Result<Self> {
            unreachable!()
        }

        unsafe fn decode_val<K: TransactionKind>(
            txn: *const ffi::MDBX_txn,
            value: ffi::MDBX_val,
        ) -> reth_libmdbx::Result<Self> {
            // SAFETY: MDBX supplies the original value and its live transaction.
            Ok(Self(unsafe { prefetch_range::<K>(txn, value) }))
        }
    }

    #[test]
    fn prefetches_clean_bytecode_in_read_only_and_writable_transactions() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = DatabaseEnv::open(
            dir.path(),
            DatabaseEnvKind::RW,
            DatabaseArguments::new(ClientVersion::default()),
        )
        .unwrap();
        db.create_tables().unwrap();
        // Larger than an OS page even on systems with 64 KiB pages.
        let code = Bytecode::new_raw(vec![0x5b; 6 * *PAGE_SIZE].into());
        let key = B256::repeat_byte(1);
        let small_key = B256::repeat_byte(2);
        let small_code = Bytecode::new_raw(vec![0].into());
        let tx = db.tx_mut().unwrap();
        let dbi = tx.get_dbi::<tables::Bytecodes>().unwrap();
        DbTxMut::put::<tables::Bytecodes>(&tx, key, code.clone()).unwrap();
        DbTxMut::put::<tables::Bytecodes>(&tx, small_key, small_code.clone()).unwrap();
        assert!(tx
            .inner()
            .get::<PrefetchRange>(dbi, key.encode().as_ref())
            .unwrap()
            .unwrap()
            .0
            .is_none());
        assert_eq!(tx.get::<tables::Bytecodes>(key).unwrap(), Some(code.clone()));
        tx.commit().unwrap();

        // Catch-up execution reads clean, mapped values through a writable transaction.
        let tx = db.tx_mut().unwrap();
        let range = tx
            .inner()
            .get::<PrefetchRange>(dbi, key.encode().as_ref())
            .unwrap()
            .unwrap()
            .0
            .unwrap();
        assert_eq!(range.0 % *PAGE_SIZE, 0);
        assert!(range.1 >= code.original_byte_slice().len());
        assert_eq!(
            tx.get_by_encoded_key::<tables::Bytecodes>(&key.encode()).unwrap(),
            Some(code.clone())
        );
        // A value becomes ineligible again when overwritten in the same transaction.
        let replacement = Bytecode::new_raw(vec![0; 6 * *PAGE_SIZE].into());
        DbTxMut::put::<tables::Bytecodes>(&tx, key, replacement.clone()).unwrap();
        assert!(tx
            .inner()
            .get::<PrefetchRange>(dbi, key.encode().as_ref())
            .unwrap()
            .unwrap()
            .0
            .is_none());
        assert_eq!(tx.get::<tables::Bytecodes>(key).unwrap(), Some(replacement));
        tx.abort();

        let tx = db.tx().unwrap();
        assert!(tx
            .inner()
            .get::<PrefetchRange>(dbi, key.encode().as_ref())
            .unwrap()
            .unwrap()
            .0
            .is_some());
        assert_eq!(tx.get::<tables::Bytecodes>(key).unwrap(), Some(code));
        assert!(tx
            .inner()
            .get::<PrefetchRange>(dbi, small_key.encode().as_ref())
            .unwrap()
            .unwrap()
            .0
            .is_none());
        assert_eq!(tx.get::<tables::Bytecodes>(small_key).unwrap(), Some(small_code));
        assert!(tx
            .inner()
            .get::<PrefetchRange>(dbi, B256::ZERO.encode().as_ref())
            .unwrap()
            .is_none());
        assert_eq!(tx.get::<tables::Bytecodes>(B256::ZERO).unwrap(), None);
    }
}

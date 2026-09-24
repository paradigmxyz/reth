//! Targeted read-ahead for mapped table values spanning multiple OS pages.

use reth_libmdbx::{ffi, TableObject, TransactionKind};
use std::{borrow::Cow, sync::LazyLock};

/// Hints a mapped table value's pages before returning its bytes for decoding.
pub(super) struct PrefetchValue<'a>(pub(super) Cow<'a, [u8]>);

impl<'a> TableObject for PrefetchValue<'a> {
    fn decode(_: &[u8]) -> reth_libmdbx::Result<Self> {
        unreachable!("value prefetch requires the MDBX transaction context")
    }

    unsafe fn decode_val<K: TransactionKind>(
        txn: *const ffi::MDBX_txn,
        value: ffi::MDBX_val,
    ) -> reth_libmdbx::Result<Self> {
        #[cfg(test)]
        tests::PREFETCH_READS.with(|count| count.set(count.get() + 1));
        // SAFETY: MDBX invokes this decoder with a value from the live transaction.
        if let Some((start, len)) = unsafe { prefetch_range::<K>(txn, value) } {
            // MDBX disables general read-ahead. Hint only this value's pages to avoid
            // serial faults while decoding large values, without enabling general read-ahead.
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
    // Unchanged values can also be read through writable transactions.
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
    use crate::{
        mdbx::DatabaseArguments, tables, test_utils::create_test_rw_db, DatabaseEnv,
        DatabaseEnvKind,
    };
    use alloy_primitives::{Address, B256, U256};
    use reth_db_api::{
        cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO},
        database::Database,
        models::{ClientVersion, ShardedKey},
        table::Encode,
        transaction::{DbTx, DbTxMut},
    };
    use reth_primitives_traits::{Bytecode, StorageEntry};
    use std::cell::Cell;

    thread_local! {
        pub(super) static PREFETCH_READS: Cell<usize> = const { Cell::new(0) };
    }

    /// Verifies decoder selection, including for small values whose advice is skipped.
    fn with_prefetch_reads<R>(expected: usize, f: impl FnOnce() -> R) -> R {
        let before = PREFETCH_READS.get();
        let result = f();
        assert_eq!(PREFETCH_READS.get() - before, expected);
        result
    }

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

    /// Inspects the same prefetch eligibility decision used by the value decoder.
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
        .unwrap()
        .with_metrics();
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
        assert_eq!(tx.prefetch().get::<tables::Bytecodes>(key).unwrap(), Some(code.clone()));
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
            tx.prefetch().get_by_encoded_key::<tables::Bytecodes>(&key.encode()).unwrap(),
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
        assert_eq!(tx.prefetch().get::<tables::Bytecodes>(key).unwrap(), Some(replacement));
        tx.abort();

        let tx = db.tx().unwrap();
        assert!(tx
            .inner()
            .get::<PrefetchRange>(dbi, key.encode().as_ref())
            .unwrap()
            .unwrap()
            .0
            .is_some());
        assert_eq!(tx.prefetch().get::<tables::Bytecodes>(key).unwrap(), Some(code));
        assert!(tx
            .inner()
            .get::<PrefetchRange>(dbi, small_key.encode().as_ref())
            .unwrap()
            .unwrap()
            .0
            .is_none());
        assert_eq!(tx.prefetch().get::<tables::Bytecodes>(small_key).unwrap(), Some(small_code));
        assert!(tx
            .inner()
            .get::<PrefetchRange>(dbi, B256::ZERO.encode().as_ref())
            .unwrap()
            .is_none());
        assert_eq!(tx.prefetch().get::<tables::Bytecodes>(B256::ZERO).unwrap(), None);
    }

    #[test]
    fn prefetched_reads_preserve_decode_errors() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = DatabaseEnv::open(
            dir.path(),
            DatabaseEnvKind::RW,
            DatabaseArguments::new(ClientVersion::default()),
        )
        .unwrap();
        db.create_tables().unwrap();
        let key = ShardedKey::new(Address::ZERO, 1);
        // One RoaringTreemap entry with an invalid bitmap header.
        let mut invalid_history = vec![0; 6 * *PAGE_SIZE];
        invalid_history[..8].copy_from_slice(&1u64.to_le_bytes());
        let tx = db.tx_mut().unwrap();
        tx.put::<tables::RawTable<tables::AccountsHistory>>(
            tables::RawKey::new(key.clone()),
            tables::RawValue::from_vec(invalid_history),
        )
        .unwrap();
        tx.commit().unwrap();

        let tx = db.tx().unwrap();
        with_prefetch_reads(1, || {
            assert_eq!(
                tx.prefetch().get::<tables::AccountsHistory>(key.clone()).unwrap_err().to_string(),
                tx.get::<tables::AccountsHistory>(key).unwrap_err().to_string()
            )
        });
    }

    #[test]
    fn transaction_prefetch_is_consumed_by_get_or_cursor() {
        let db = create_test_rw_db();
        let tx = db.tx_mut().unwrap();
        let key = B256::repeat_byte(1);
        let code = Bytecode::new_raw(vec![0x5b; 6 * *PAGE_SIZE].into());
        assert!(std::ptr::eq(tx.prefetch(), &raw const tx));
        // Writes and statistics leave the pending request intact.
        DbTxMut::put::<tables::Bytecodes>(&tx, key, code.clone()).unwrap();
        assert_eq!(tx.entries::<tables::Bytecodes>().unwrap(), 1);
        with_prefetch_reads(1, || {
            assert_eq!(tx.get::<tables::Bytecodes>(key).unwrap(), Some(code.clone()))
        });
        with_prefetch_reads(0, || tx.get::<tables::Bytecodes>(key).unwrap());

        // A miss also consumes the request, without invoking the value decoder.
        with_prefetch_reads(0, || {
            assert!(tx.prefetch().get::<tables::Bytecodes>(B256::ZERO).unwrap().is_none());
            tx.get::<tables::Bytecodes>(key).unwrap();
        });
        let mut ordinary = tx.cursor_read::<tables::Bytecodes>().unwrap();
        let mut prefetched = tx.prefetch().cursor_write::<tables::Bytecodes>().unwrap();
        with_prefetch_reads(0, || {
            ordinary.first().unwrap();
            tx.get::<tables::Bytecodes>(key).unwrap();
        });
        with_prefetch_reads(1, || assert_eq!(prefetched.first().unwrap(), Some((key, code))));
        // Writes through an opted-in cursor keep its read-ahead setting.
        prefetched.upsert(key, &Bytecode::new_raw(vec![0].into())).unwrap();
        with_prefetch_reads(1, || prefetched.current().unwrap());
    }

    #[test]
    fn prefetch_covers_cursor_reads_and_walks() {
        let db = create_test_rw_db();
        let tx = db.tx_mut().unwrap();
        let code = Bytecode::new_raw(vec![0x5b; 6 * *PAGE_SIZE].into());
        let keys = [B256::repeat_byte(1), B256::repeat_byte(2), B256::repeat_byte(3)];
        for key in keys {
            DbTxMut::put::<tables::Bytecodes>(&tx, key, code.clone()).unwrap();
        }
        tx.commit().unwrap();
        let tx = db.tx().unwrap();
        let mut cursor = tx.prefetch().cursor_read::<tables::Bytecodes>().unwrap();
        with_prefetch_reads(7, || {
            assert_eq!(cursor.first().unwrap(), Some((keys[0], code.clone())));
            assert_eq!(cursor.next().unwrap(), Some((keys[1], code.clone())));
            assert_eq!(cursor.prev().unwrap(), Some((keys[0], code.clone())));
            assert_eq!(cursor.last().unwrap(), Some((keys[2], code.clone())));
            assert_eq!(cursor.current().unwrap(), Some((keys[2], code.clone())));
            assert_eq!(cursor.seek(keys[1]).unwrap(), Some((keys[1], code.clone())));
            assert_eq!(cursor.seek_exact(keys[0]).unwrap(), Some((keys[0], code.clone())));
        });
        let expected = keys.map(|key| (key, code.clone())).to_vec();
        for start in [None, Some(keys[0])] {
            with_prefetch_reads(3, || {
                assert_eq!(
                    cursor.walk(start).unwrap().collect::<Result<Vec<_>, _>>().unwrap(),
                    expected
                );
            });
        }
        for start in [None, Some(keys[2])] {
            with_prefetch_reads(3, || {
                assert_eq!(
                    cursor.walk_back(start).unwrap().collect::<Result<Vec<_>, _>>().unwrap(),
                    expected.iter().rev().cloned().collect::<Vec<_>>()
                );
            });
        }
        with_prefetch_reads(3, || {
            assert_eq!(
                cursor
                    .walk_range(keys[0]..=keys[2])
                    .unwrap()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap(),
                expected
            );
        });
        with_prefetch_reads(3, || {
            assert_eq!(
                cursor.walk_range(..).unwrap().collect::<Result<Vec<_>, _>>().unwrap(),
                expected
            );
        });
    }

    #[test]
    fn prefetch_covers_duplicate_cursor_reads() {
        let db = create_test_rw_db();
        let tx = db.tx_mut().unwrap();
        let address = Address::repeat_byte(1);
        let next_address = Address::repeat_byte(2);
        let entries =
            [1, 2, 3].map(|i| StorageEntry { key: B256::repeat_byte(i), value: U256::from(i) });
        for entry in entries {
            DbTxMut::put::<tables::PlainStorageState>(&tx, address, entry).unwrap();
        }
        DbTxMut::put::<tables::PlainStorageState>(&tx, next_address, entries[0]).unwrap();
        tx.commit().unwrap();
        let tx = db.tx_mut().unwrap();
        // Both read-only and writable duplicate cursor factories inherit the flag.
        let mut cursor = tx.prefetch().cursor_dup_read::<tables::PlainStorageState>().unwrap();
        with_prefetch_reads(7, || {
            assert_eq!(cursor.first().unwrap(), Some((address, entries[0])));
            assert_eq!(cursor.next_dup().unwrap(), Some((address, entries[1])));
            assert_eq!(cursor.prev_dup().unwrap(), Some((address, entries[0])));
            assert_eq!(cursor.next_dup_val().unwrap(), Some(entries[1]));
            assert_eq!(cursor.last_dup().unwrap(), Some(entries[2]));
            assert_eq!(cursor.next_no_dup().unwrap(), Some((next_address, entries[0])));
            assert_eq!(
                cursor.seek_by_key_subkey(address, entries[1].key).unwrap(),
                Some(entries[1])
            );
        });
        let mut cursor = tx.prefetch().cursor_dup_write::<tables::PlainStorageState>().unwrap();
        for (key, subkey, reads) in [
            (Some(address), Some(entries[0].key), 3),
            (Some(address), None, 3),
            (None, Some(entries[0].key), 4),
            (None, None, 3),
        ] {
            with_prefetch_reads(reads, || {
                assert_eq!(
                    cursor.walk_dup(key, subkey).unwrap().collect::<Result<Vec<_>, _>>().unwrap(),
                    entries.map(|entry| (address, entry)).to_vec()
                );
            });
        }
    }
}

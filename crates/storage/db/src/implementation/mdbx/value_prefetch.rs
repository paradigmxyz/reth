//! Targeted read-ahead for mapped table values spanning multiple OS pages.

use reth_libmdbx::{ffi, TableObject, TransactionKind};
use std::borrow::Cow;

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

/// Returns the OS pages to prefetch, excluding small and dirty values.
///
/// # Safety
/// `value` must be the unmodified value returned by MDBX for the live `txn` of kind `K`.
unsafe fn prefetch_range<K: TransactionKind>(
    txn: *const ffi::MDBX_txn,
    value: ffi::MDBX_val,
) -> Option<(usize, usize)> {
    let page_size = page_size::get();
    if value.iov_len <= page_size {
        return None;
    }
    // Unchanged values can also be read through writable transactions.
    // Dirty values can be heap-backed; never advise them, even with `return-borrowed`.
    // SAFETY: This is the original value pointer, before any copy or transaction mutation.
    if !K::IS_READ_ONLY && unsafe { ffi::mdbx_is_dirty(txn, value.iov_base) } != ffi::MDBX_SUCCESS {
        return None;
    }
    page_range(value.iov_base as usize, value.iov_len, page_size)
}

/// Rounds a nonempty value's range to complete pages, rejecting invalid sizes and overflow.
fn page_range(address: usize, len: usize, page_size: usize) -> Option<(usize, usize)> {
    if len == 0 || !page_size.is_power_of_two() {
        return None;
    }
    let start = address & !(page_size - 1);
    let end = address.checked_add(len)?.checked_add(page_size - 1)? & !(page_size - 1);
    Some((start, end.checked_sub(start)?))
}

/// MDBX's main table, whose records hold each named table's B-tree descriptor (`tree_t`).
const MAIN_DBI: ffi::MDBX_dbi = 1;
/// Size of an MDBX page header (`PAGEHDRSZ`), followed by 16-bit node offsets.
const PAGE_HEADER: usize = 20;
/// Size of an MDBX node header; branch nodes start with their child page number.
const NODE_HEADER: usize = 8;
/// MDBX page flag of branch pages (`P_BRANCH`).
const P_BRANCH: u16 = 0x01;
/// MDBX page flag of leaf pages (`P_LEAF`).
const P_LEAF: u16 = 0x02;

/// Advises the kernel to read the leaf pages holding `keys` in `table` before they are looked up.
///
/// Cold lookups fault one page at a time, so a batch of N cold keys waits for N serial reads.
/// This descends the table's B-tree for all keys together, one level at a time: each level's
/// pages are advised with `MADV_WILLNEED`, which queues their reads, before the level is read to
/// find the next one. A batch then waits for about one read per tree level, and the leaf reads are
/// still in flight when this returns.
///
/// Duplicate subtrees are not followed. Advice is best-effort: an unexpected page layout ends the
/// walk without an error.
///
/// # Safety
/// `txn` must be a live read-only MDBX transaction.
pub(super) unsafe fn prefetch_keys<Key: AsRef<[u8]>>(
    txn: *mut ffi::MDBX_txn,
    table: &str,
    keys: &[Key],
) {
    // SAFETY: The caller guarantees a live read-only transaction.
    let _ = unsafe { leaf_pages(txn, table, keys) };
}

/// Advises each level of `table`'s B-tree for `keys` and returns their leaf page numbers.
///
/// # Safety
/// `txn` must be a live read-only MDBX transaction.
unsafe fn leaf_pages<Key: AsRef<[u8]>>(
    txn: *mut ffi::MDBX_txn,
    table: &str,
    keys: &[Key],
) -> Option<Vec<usize>> {
    // SAFETY: The caller guarantees a live read-only transaction.
    let tree = unsafe { Tree::open(txn, table)? };
    let mut pages = vec![tree.root; keys.len()];
    let mut level = Vec::with_capacity(keys.len());
    for depth in 1..=tree.height {
        level.clear();
        level.extend_from_slice(&pages);
        level.sort_unstable();
        level.dedup();
        tree.advise(&level);
        if depth < tree.height {
            // Reads of this level's pages were queued above, so they complete concurrently.
            for (page, key) in pages.iter_mut().zip(keys) {
                *page = branch_child(tree.page(*page, P_BRANCH)?, key.as_ref())?;
            }
        }
    }
    Some(pages)
}

/// A table's B-tree within the memory map of a read-only transaction's snapshot.
struct Tree {
    /// Address of page 0.
    base: usize,
    /// MDBX page size, equal to the OS page size.
    page_size: usize,
    /// Number of pages in the data file.
    pages: usize,
    /// Root page number.
    root: usize,
    /// Number of levels, including the leaves.
    height: usize,
}

impl Tree {
    /// Reads `table`'s committed descriptor and locates the memory map.
    ///
    /// # Safety
    /// `txn` must be a live read-only MDBX transaction.
    unsafe fn open(txn: *mut ffi::MDBX_txn, table: &str) -> Option<Self> {
        let page_size = page_size::get();
        // Page and node headers are only parsed in their little-endian layout.
        if cfg!(target_endian = "big") || !page_size.is_power_of_two() {
            return None;
        }
        // SAFETY: An all-zero `MDBX_envinfo` is valid, and MDBX fills it for the live `txn`.
        let mut info: ffi::MDBX_envinfo = unsafe { std::mem::zeroed() };
        if unsafe {
            ffi::mdbx_env_info_ex(
                ffi::mdbx_txn_env(txn),
                txn,
                &raw mut info,
                size_of::<ffi::MDBX_envinfo>(),
            )
        } != ffi::MDBX_SUCCESS ||
            info.mi_dxb_pagesize as usize != page_size
        {
            return None;
        }
        let name = ffi::MDBX_val { iov_len: table.len(), iov_base: table.as_ptr() as *mut _ };
        let mut record = ffi::MDBX_val { iov_len: 0, iov_base: std::ptr::null_mut() };
        // SAFETY: MDBX only reads the key, and read-only values point into the memory map, which
        // stays valid while `txn` is live.
        let record = unsafe {
            if ffi::mdbx_get(txn, MAIN_DBI, &raw const name, &raw mut record) != ffi::MDBX_SUCCESS {
                return None;
            }
            std::slice::from_raw_parts(record.iov_base as *const u8, record.iov_len)
        };
        // The record's own leaf page locates the map, since MDBX pages match OS pages and the map
        // is OS page aligned.
        let leaf = record.as_ptr() as usize & !(page_size - 1);
        // SAFETY: The record lies within this mapped page.
        let header = unsafe { std::slice::from_raw_parts(leaf as *const u8, PAGE_HEADER) };
        let leaf_pgno = read_u32(header, 16)? as usize;
        let tree = Self {
            base: leaf.checked_sub(leaf_pgno.checked_mul(page_size)?)?,
            page_size,
            pages: usize::try_from(info.mi_geo.current).ok()? / page_size,
            root: read_u32(record, 8)? as usize,
            height: usize::from(read_u16(record, 2)?),
        };
        tree.page(leaf_pgno, P_LEAF)?;
        Some(tree)
    }

    /// Returns page `pgno` if it is within the data file and its header has `flag` and `pgno`.
    fn page(&self, pgno: usize, flag: u16) -> Option<&[u8]> {
        if pgno >= self.pages {
            return None;
        }
        // SAFETY: Pages below the data file size are mapped. Pages reachable from the snapshot's
        // root stay unchanged while it is live, and this only follows such pages, validating each
        // header so that unexpected layouts end the walk.
        let page = unsafe {
            std::slice::from_raw_parts(
                (self.base + pgno * self.page_size) as *const u8,
                self.page_size,
            )
        };
        (read_u16(page, 10)? & flag != 0 && read_u32(page, 16)? as usize == pgno).then_some(page)
    }

    /// Advises `pages`, sorted and unique, coalescing consecutive pages into one call.
    fn advise(&self, pages: &[usize]) {
        for run in pages.chunk_by(|a, b| a + 1 == *b) {
            if run[run.len() - 1] >= self.pages {
                continue;
            }
            // SAFETY: The range lies within the data file mapping. Advice is best-effort and
            // doesn't change the mapped contents.
            let _ = unsafe {
                libc::madvise(
                    (self.base + run[0] * self.page_size) as *mut libc::c_void,
                    run.len() * self.page_size,
                    libc::MADV_WILLNEED,
                )
            };
        }
    }
}

/// Returns the child page number of branch `page` whose key range contains `key`.
///
/// The first node's key is implicitly lower than every key. Other keys use MDBX's default
/// lexicographic order, which matches slice ordering.
fn branch_child(page: &[u8], key: &[u8]) -> Option<usize> {
    let count = usize::from(read_u16(page, 12)?) / 2;
    let node =
        |index: usize| Some(PAGE_HEADER + usize::from(read_u16(page, PAGE_HEADER + 2 * index)?));
    let node_key = |index: usize| {
        let node = node(index)?;
        page.get(node + NODE_HEADER..node + NODE_HEADER + usize::from(read_u16(page, node + 6)?))
    };
    if count == 0 {
        return None;
    }
    let (mut low, mut high) = (1, count);
    while low < high {
        let mid = low + (high - low) / 2;
        if node_key(mid)? <= key {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    Some(read_u32(page, node(low - 1)?)? as usize)
}

/// Reads a little-endian `u16` at `offset`.
fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(offset..offset.checked_add(2)?)?.try_into().ok()?))
}

/// Reads a little-endian `u32` at `offset`.
fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?))
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
        table::{Encode, Table},
        transaction::{DbTx, DbTxMut},
    };
    use reth_primitives_traits::{Account, Bytecode, StorageEntry};
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
    fn prefetch_keys_resolves_leaf_pages() {
        let db = create_test_rw_db();
        let keys: Vec<B256> =
            (0..20_000u64).map(|i| alloy_primitives::keccak256(i.to_be_bytes())).collect();
        let tx = db.tx_mut().unwrap();
        for (i, key) in keys.iter().enumerate() {
            let account = Account { nonce: i as u64, ..Default::default() };
            tx.put::<tables::HashedAccounts>(*key, account).unwrap();
        }
        let encoded: Vec<_> = keys.iter().map(|key| key.encode()).collect();
        // Writable transactions are ignored.
        tx.prefetch_keys::<tables::HashedAccounts>(&encoded);
        tx.commit().unwrap();

        let tx = db.tx().unwrap();
        let dbi = tx.get_dbi::<tables::HashedAccounts>().unwrap();
        assert!(tx.inner().db_stat_with_dbi(dbi).unwrap().depth() >= 3);
        let missing = [B256::ZERO, B256::repeat_byte(0xff)];
        let probes: Vec<B256> = keys.iter().step_by(7).chain(&missing).copied().collect();
        let leaves = tx
            .inner()
            .txn_execute(|txn| unsafe { leaf_pages(txn, tables::HashedAccounts::NAME, &probes) })
            .unwrap()
            .unwrap();
        let page_size = page_size::get();
        for (key, leaf) in probes.iter().zip(leaves) {
            let mut cursor = tx.inner().cursor_with_dbi(dbi).unwrap();
            // Missing keys resolve to the leaf where a lookup positions the cursor.
            let (found, _): (Cow<'_, [u8]>, Cow<'_, [u8]>) = cursor
                .set_range(key.as_slice())
                .unwrap()
                .unwrap_or_else(|| cursor.last().unwrap().unwrap());
            let Cow::Borrowed(found) = found else { panic!("read-only keys are borrowed") };
            let page = found.as_ptr() as usize & !(page_size - 1);
            // SAFETY: The page header is mapped while the transaction is live.
            let pgno = unsafe { *((page + 16) as *const u32) } as usize;
            assert_eq!(leaf, pgno);
        }
        tx.prefetch_keys::<tables::HashedAccounts>(&encoded);
        assert_eq!(tx.entries::<tables::HashedAccounts>().unwrap(), keys.len());
    }

    #[test]
    fn rounds_to_os_pages() {
        assert_eq!(page_range(0x1014, 27_693, 4096), Some((0x1000, 7 * 4096)));
        assert_eq!(page_range(0x1000, 4096, 4096), Some((0x1000, 4096)));
        assert_eq!(page_range(0x1fff, 2, 4096), Some((0x1000, 8192)));
        assert_eq!(page_range(0x1014, 0xfec, 4096), Some((0x1000, 4096)));
        assert_eq!(page_range(0x1ffff, 2, 65536), Some((0x10000, 2 * 65536)));
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
        let code = Bytecode::new_raw(vec![0x5b; 6 * page_size::get()].into());
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
        assert_eq!(tx.prefetch(true).get::<tables::Bytecodes>(key).unwrap(), Some(code.clone()));
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
        assert_eq!(range.0 % page_size::get(), 0);
        assert!(range.1 >= code.original_byte_slice().len());
        assert_eq!(
            tx.prefetch(true).get_by_encoded_key::<tables::Bytecodes>(&key.encode()).unwrap(),
            Some(code.clone())
        );
        // A value becomes ineligible again when overwritten in the same transaction.
        let replacement = Bytecode::new_raw(vec![0; 6 * page_size::get()].into());
        DbTxMut::put::<tables::Bytecodes>(&tx, key, replacement.clone()).unwrap();
        assert!(tx
            .inner()
            .get::<PrefetchRange>(dbi, key.encode().as_ref())
            .unwrap()
            .unwrap()
            .0
            .is_none());
        assert_eq!(tx.prefetch(true).get::<tables::Bytecodes>(key).unwrap(), Some(replacement));
        tx.abort();

        let tx = db.tx().unwrap();
        assert!(tx
            .inner()
            .get::<PrefetchRange>(dbi, key.encode().as_ref())
            .unwrap()
            .unwrap()
            .0
            .is_some());
        assert_eq!(tx.prefetch(true).get::<tables::Bytecodes>(key).unwrap(), Some(code));
        assert!(tx
            .inner()
            .get::<PrefetchRange>(dbi, small_key.encode().as_ref())
            .unwrap()
            .unwrap()
            .0
            .is_none());
        assert_eq!(
            tx.prefetch(true).get::<tables::Bytecodes>(small_key).unwrap(),
            Some(small_code)
        );
        assert!(tx
            .inner()
            .get::<PrefetchRange>(dbi, B256::ZERO.encode().as_ref())
            .unwrap()
            .is_none());
        assert_eq!(tx.prefetch(true).get::<tables::Bytecodes>(B256::ZERO).unwrap(), None);
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
        let mut invalid_history = vec![0; 6 * page_size::get()];
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
                tx.prefetch(true)
                    .get::<tables::AccountsHistory>(key.clone())
                    .unwrap_err()
                    .to_string(),
                tx.prefetch(false)
                    .get::<tables::AccountsHistory>(key.clone())
                    .unwrap_err()
                    .to_string()
            )
        });
        tx.prefetch(true);
        with_prefetch_reads(2, || {
            assert!(tx.get::<tables::AccountsHistory>(key.clone()).is_err());
            assert!(tx.get::<tables::AccountsHistory>(key).is_err());
        });
    }

    #[test]
    fn transaction_and_cursor_prefetch_persist_until_changed() {
        let db = create_test_rw_db();
        let tx = db.tx_mut().unwrap();
        let key = B256::repeat_byte(1);
        let code = Bytecode::new_raw(vec![0x5b; 6 * page_size::get()].into());
        let mut ordinary = tx.cursor_read::<tables::Bytecodes>().unwrap();
        assert!(std::ptr::eq(tx.prefetch(true), &raw const tx));
        // Writes, statistics, gets, misses, and cursor creation preserve the setting.
        DbTxMut::put::<tables::Bytecodes>(&tx, key, code.clone()).unwrap();
        assert_eq!(tx.entries::<tables::Bytecodes>().unwrap(), 1);
        with_prefetch_reads(2, || {
            assert_eq!(tx.get::<tables::Bytecodes>(key).unwrap(), Some(code.clone()));
            assert_eq!(
                tx.get_by_encoded_key::<tables::Bytecodes>(&key).unwrap(),
                Some(code.clone())
            );
        });
        with_prefetch_reads(1, || {
            assert!(tx.get::<tables::Bytecodes>(B256::ZERO).unwrap().is_none());
            tx.get::<tables::Bytecodes>(key).unwrap();
        });
        let mut prefetched = tx.cursor_write::<tables::Bytecodes>().unwrap();
        let mut sibling = tx.cursor_read::<tables::Bytecodes>().unwrap();
        with_prefetch_reads(0, || ordinary.first().unwrap());
        with_prefetch_reads(3, || {
            assert_eq!(prefetched.first().unwrap(), Some((key, code)));
            sibling.first().unwrap();
            tx.get::<tables::Bytecodes>(key).unwrap();
        });

        // A cursor can disable its inherited setting without changing its position or siblings.
        with_prefetch_reads(0, || prefetched.prefetch(false).current().unwrap());
        with_prefetch_reads(2, || {
            sibling.current().unwrap();
            tx.get::<tables::Bytecodes>(key).unwrap();
        });
        tx.prefetch(false);
        with_prefetch_reads(0, || {
            tx.get::<tables::Bytecodes>(key).unwrap();
            tx.cursor_read::<tables::Bytecodes>().unwrap().first().unwrap();
        });
        with_prefetch_reads(1, || sibling.current().unwrap());

        // Cursor overrides are independent of the transaction and persist through writes.
        with_prefetch_reads(1, || prefetched.prefetch(true).current().unwrap());
        prefetched.upsert(key, &Bytecode::new_raw(vec![0].into())).unwrap();
        with_prefetch_reads(1, || prefetched.current().unwrap());
        with_prefetch_reads(0, || {
            ordinary.current().unwrap();
            tx.get::<tables::Bytecodes>(key).unwrap();
        });
    }

    #[test]
    fn prefetch_covers_cursor_reads_and_walks() {
        let db = create_test_rw_db();
        let tx = db.tx_mut().unwrap();
        let code = Bytecode::new_raw(vec![0x5b; 6 * page_size::get()].into());
        let keys = [B256::repeat_byte(1), B256::repeat_byte(2), B256::repeat_byte(3)];
        for key in keys {
            DbTxMut::put::<tables::Bytecodes>(&tx, key, code.clone()).unwrap();
        }
        tx.commit().unwrap();
        let tx = db.tx().unwrap();
        let mut cursor = tx.prefetch(true).cursor_read::<tables::Bytecodes>().unwrap();
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
        with_prefetch_reads(0, || {
            assert_eq!(
                cursor.prefetch(false).walk(None).unwrap().collect::<Result<Vec<_>, _>>().unwrap(),
                expected
            );
        });
        with_prefetch_reads(3, || {
            assert_eq!(
                cursor.prefetch(true).walk(None).unwrap().collect::<Result<Vec<_>, _>>().unwrap(),
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
        let mut cursor = tx.prefetch(true).cursor_dup_read::<tables::PlainStorageState>().unwrap();
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
        let mut cursor = tx.prefetch(true).cursor_dup_write::<tables::PlainStorageState>().unwrap();
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
        with_prefetch_reads(0, || {
            assert_eq!(
                cursor.prefetch(false).seek_by_key_subkey(address, entries[0].key).unwrap(),
                Some(entries[0])
            );
            assert_eq!(cursor.next_dup_val().unwrap(), Some(entries[1]));
        });
        with_prefetch_reads(2, || {
            assert_eq!(
                cursor.prefetch(true).seek_by_key_subkey(address, entries[0].key).unwrap(),
                Some(entries[0])
            );
            assert_eq!(cursor.next_dup_val().unwrap(), Some(entries[1]));
        });
    }
}

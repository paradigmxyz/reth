//! Invariant tests for MDBX parallel subtransactions.
//!
//! These tests verify safety invariants that must hold to prevent corruption.
//! They are designed to catch regressions during refactors and integrations.

#![allow(missing_docs)]
use reth_libmdbx::*;
use std::{borrow::Cow, sync::Arc, thread};
use tempfile::tempdir;

// =============================================================================
// INVARIANT 1: 1 DBI = 1 SUBTXN - Cross-DBI access must fail
// =============================================================================

#[test]
fn test_invariant_cross_dbi_access_rejected() {
    // When parallel writes is enabled, accessing a DBI without a subtxn must fail.
    let dir = tempdir().unwrap();
    let env = Environment::builder()
        .set_max_dbs(10)
        .set_geometry(Geometry { size: Some(0..(1024 * 1024 * 10)), ..Default::default() })
        .write_map()
        .open(dir.path())
        .unwrap();

    let txn = env.begin_rw_txn().unwrap();
    let db1 = txn.create_db(Some("table1"), DatabaseFlags::empty()).unwrap();
    let db2 = txn.create_db(Some("table2"), DatabaseFlags::empty()).unwrap();
    let dbi1 = db1.dbi();
    let dbi2 = db2.dbi();
    txn.commit().unwrap();

    let mut txn = env.begin_rw_txn().unwrap();
    // Only enable subtxn for dbi1
    txn.enable_parallel_writes(&[dbi1]).unwrap();

    // Get cursor for dbi1 - should work
    {
        let cursor1 = txn.cursor_with_dbi_parallel(dbi1);
        assert!(cursor1.is_ok(), "Cursor for assigned DBI should succeed");
    } // cursor1 dropped here before abort

    // Try to get cursor for dbi2 - should FAIL (no subtxn for it)
    {
        let cursor2 = txn.cursor_with_dbi_parallel(dbi2);
        assert!(cursor2.is_err(), "Cursor for non-assigned DBI should fail");
    }

    txn.abort_subtxns().unwrap();
    drop(txn);
}

#[test]
fn test_invariant_dbi_subtxn_isolation() {
    // Each subtxn operates on its assigned DBI - verify data isolation.
    let dir = tempdir().unwrap();
    let env = Environment::builder()
        .set_max_dbs(10)
        .set_geometry(Geometry { size: Some(0..(1024 * 1024 * 10)), ..Default::default() })
        .write_map()
        .open(dir.path())
        .unwrap();

    let txn = env.begin_rw_txn().unwrap();
    let db1 = txn.create_db(Some("table1"), DatabaseFlags::empty()).unwrap();
    let db2 = txn.create_db(Some("table2"), DatabaseFlags::empty()).unwrap();
    let dbi1 = db1.dbi();
    let dbi2 = db2.dbi();
    txn.commit().unwrap();

    let mut txn = env.begin_rw_txn().unwrap();
    // Enable subtxn for BOTH DBIs - proper usage pattern
    txn.enable_parallel_writes(&[dbi1, dbi2]).unwrap();

    {
        // Get cursor for dbi1 - should work via subtxn
        let cursor1 = txn.cursor_with_dbi_parallel(dbi1);
        assert!(cursor1.is_ok(), "Cursor for assigned DBI 1 should succeed");

        // Get cursor for dbi2 - should work via its own subtxn
        let cursor2 = txn.cursor_with_dbi_parallel(dbi2);
        assert!(cursor2.is_ok(), "Cursor for assigned DBI 2 should succeed");

        // Write to both
        cursor1.unwrap().put(b"key1", b"val1", WriteFlags::empty()).unwrap();
        cursor2.unwrap().put(b"key2", b"val2", WriteFlags::empty()).unwrap();
    }

    txn.commit_subtxns().unwrap();
    txn.commit().unwrap();

    // Verify isolation - data written via separate subtxns
    let txn = env.begin_ro_txn().unwrap();
    let v1: Option<Cow<'_, [u8]>> = txn.get(dbi1, b"key1").unwrap();
    let v2: Option<Cow<'_, [u8]>> = txn.get(dbi2, b"key2").unwrap();
    assert_eq!(v1.as_deref(), Some(b"val1".as_slice()));
    assert_eq!(v2.as_deref(), Some(b"val2".as_slice()));
}

#[test]
fn test_invariant_duplicate_dbi_in_subtxn_list_rejected() {
    let dir = tempdir().unwrap();
    let env = Environment::builder()
        .set_max_dbs(10)
        .set_geometry(Geometry { size: Some(0..(1024 * 1024 * 10)), ..Default::default() })
        .write_map()
        .open(dir.path())
        .unwrap();

    let txn = env.begin_rw_txn().unwrap();
    let db = txn.create_db(Some("table"), DatabaseFlags::empty()).unwrap();
    let dbi = db.dbi();
    txn.commit().unwrap();

    let txn = env.begin_rw_txn().unwrap();
    // Try to create subtxns with duplicate DBI
    let result = txn.enable_parallel_writes(&[dbi, dbi]);
    assert!(result.is_err(), "Duplicate DBI should be rejected");
    drop(txn); // Abort via drop
}

// =============================================================================
// INVARIANT 2: WRITEMAP required for parallel subtxns
// =============================================================================

#[test]
fn test_invariant_non_writemap_rejected() {
    let dir = tempdir().unwrap();
    // Create env WITHOUT write_map()
    let env = Environment::builder()
        .set_max_dbs(10)
        .set_geometry(Geometry { size: Some(0..(1024 * 1024 * 10)), ..Default::default() })
        // NO .write_map() - should fail
        .open(dir.path())
        .unwrap();

    let txn = env.begin_rw_txn().unwrap();
    let db = txn.create_db(Some("table"), DatabaseFlags::empty()).unwrap();
    let dbi = db.dbi();
    txn.commit().unwrap();

    let txn = env.begin_rw_txn().unwrap();
    let result = txn.enable_parallel_writes(&[dbi]);
    assert!(result.is_err(), "Non-WRITEMAP mode should reject parallel subtxns");
    drop(txn); // Abort via drop
}

// =============================================================================
// INVARIANT 3: All subtxns must commit before parent
// =============================================================================

#[test]
fn test_invariant_parent_commit_blocked_while_subtxns_uncommitted() {
    // Parent commit must fail if subtxns have not been committed.
    let dir = tempdir().unwrap();
    let env = Environment::builder()
        .set_max_dbs(10)
        .set_geometry(Geometry { size: Some(0..(1024 * 1024 * 10)), ..Default::default() })
        .write_map()
        .open(dir.path())
        .unwrap();

    let txn = env.begin_rw_txn().unwrap();
    let db = txn.create_db(Some("table"), DatabaseFlags::empty()).unwrap();
    let dbi = db.dbi();
    txn.commit().unwrap();

    let txn = env.begin_rw_txn().unwrap();
    txn.enable_parallel_writes(&[dbi]).unwrap();

    // Write something via subtxn
    {
        let mut cursor = txn.cursor_with_dbi_parallel(dbi).unwrap();
        cursor.put(b"key", b"value", WriteFlags::empty()).unwrap();
    }

    // Try to commit parent WITHOUT committing subtxns first - should fail
    let result = txn.commit();
    assert!(result.is_err(), "Parent commit should fail while subtxns uncommitted");
}

#[test]
fn test_invariant_commit_subtxns_then_parent_succeeds() {
    let dir = tempdir().unwrap();
    let env = Environment::builder()
        .set_max_dbs(10)
        .set_geometry(Geometry { size: Some(0..(1024 * 1024 * 10)), ..Default::default() })
        .write_map()
        .open(dir.path())
        .unwrap();

    let txn = env.begin_rw_txn().unwrap();
    let db = txn.create_db(Some("table"), DatabaseFlags::empty()).unwrap();
    let dbi = db.dbi();
    txn.commit().unwrap();

    let mut txn = env.begin_rw_txn().unwrap();
    txn.enable_parallel_writes(&[dbi]).unwrap();

    {
        let mut cursor = txn.cursor_with_dbi_parallel(dbi).unwrap();
        cursor.put(b"key", b"value", WriteFlags::empty()).unwrap();
    }

    // Commit subtxns first
    txn.commit_subtxns().unwrap();
    // Now parent commit should succeed
    txn.commit().unwrap();

    // Verify data persisted
    let txn = env.begin_ro_txn().unwrap();
    let val: Option<Cow<'_, [u8]>> = txn.get(dbi, b"key").unwrap();
    assert_eq!(val.as_deref(), Some(b"value".as_slice()));
}

// =============================================================================
// INVARIANT 4: Subtxn abort returns pages to parent
// =============================================================================

#[test]
fn test_invariant_subtxn_abort_no_page_leak() {
    let dir = tempdir().unwrap();
    let env = Environment::builder()
        .set_max_dbs(10)
        .set_geometry(Geometry { size: Some(0..(1024 * 1024 * 100)), ..Default::default() })
        .write_map()
        .open(dir.path())
        .unwrap();

    let txn = env.begin_rw_txn().unwrap();
    let db = txn.create_db(Some("table"), DatabaseFlags::empty()).unwrap();
    let dbi = db.dbi();
    txn.commit().unwrap();

    let freelist_before = env.freelist().unwrap();

    // Run multiple abort cycles - pages should not leak
    for _ in 0..10 {
        let mut txn = env.begin_rw_txn().unwrap();
        txn.enable_parallel_writes_with_hints(&[(dbi, 100)]).unwrap();

        // Allocate some pages
        {
            let mut cursor = txn.cursor_with_dbi_parallel(dbi).unwrap();
            for i in 0..500u32 {
                let key = i.to_be_bytes();
                let val = [0xAA; 200];
                cursor.put(&key, &val, WriteFlags::empty()).unwrap();
            }
        }

        // Abort subtxns (not commit)
        txn.abort_subtxns().unwrap();
        drop(txn); // Abort via drop
    }

    let freelist_after = env.freelist().unwrap();

    // Freelist should not grow significantly (pages returned on abort)
    assert!(
        freelist_after <= freelist_before + 50,
        "Freelist grew too much after aborts: {} -> {}. Page leak on abort!",
        freelist_before,
        freelist_after
    );
}

// =============================================================================
// INVARIANT 5: Each subtxn has own page_auxbuf (DupSort safety)
// =============================================================================

#[test]
fn test_invariant_dupsort_concurrent_safety() {
    let dir = tempdir().unwrap();
    let env = Arc::new(
        Environment::builder()
            .set_max_dbs(10)
            .set_geometry(Geometry { size: Some(0..(1024 * 1024 * 100)), ..Default::default() })
            .write_map()
            .open(dir.path())
            .unwrap(),
    );

    let txn = env.begin_rw_txn().unwrap();
    let db1 = txn.create_db(Some("dupsort1"), DatabaseFlags::DUP_SORT).unwrap();
    let db2 = txn.create_db(Some("dupsort2"), DatabaseFlags::DUP_SORT).unwrap();
    let dbi1 = db1.dbi();
    let dbi2 = db2.dbi();
    txn.commit().unwrap();

    // Create parent with subtxns
    let txn = env.begin_rw_txn().unwrap();
    txn.enable_parallel_writes(&[dbi1, dbi2]).unwrap();

    // Get transaction pointer for thread sharing (unsafe but necessary for test)
    let txn = Arc::new(txn);
    let txn1 = Arc::clone(&txn);
    let txn2 = Arc::clone(&txn);

    // Concurrent DupSort writes - would corrupt if sharing page_auxbuf
    let handle1 = thread::spawn(move || {
        let mut cursor = txn1.cursor_with_dbi_parallel(dbi1).unwrap();
        for i in 0..1000u32 {
            let key = (i % 10).to_be_bytes(); // 10 unique keys
            let val = format!("value1_{i:05}");
            cursor.put(&key, val.as_bytes(), WriteFlags::empty()).unwrap();
        }
    });

    let handle2 = thread::spawn(move || {
        let mut cursor = txn2.cursor_with_dbi_parallel(dbi2).unwrap();
        for i in 0..1000u32 {
            let key = (i % 10).to_be_bytes();
            let val = format!("value2_{i:05}");
            cursor.put(&key, val.as_bytes(), WriteFlags::empty()).unwrap();
        }
    });

    handle1.join().unwrap();
    handle2.join().unwrap();

    // If page_auxbuf was shared, data would be corrupted
    // Use Arc::try_unwrap to get ownership back
    let mut txn = Arc::try_unwrap(txn).expect("All thread handles joined");
    txn.commit_subtxns().unwrap();
    txn.commit().unwrap();

    // Verify data integrity
    let txn = env.begin_ro_txn().unwrap();
    let cursor1 = txn.cursor(dbi1).unwrap();
    let cursor2 = txn.cursor(dbi2).unwrap();

    let count1: usize = cursor1.iter_slices().count();
    let count2: usize = cursor2.iter_slices().count();

    assert_eq!(count1, 1000, "DupSort table 1 should have 1000 entries");
    assert_eq!(count2, 1000, "DupSort table 2 should have 1000 entries");
}

// =============================================================================
// INVARIANT 6: Commit serialization via mutex
// =============================================================================

#[test]
fn test_invariant_concurrent_commits_serialized() {
    let dir = tempdir().unwrap();
    let env = Arc::new(
        Environment::builder()
            .set_max_dbs(10)
            .set_geometry(Geometry { size: Some(0..(1024 * 1024 * 100)), ..Default::default() })
            .write_map()
            .open(dir.path())
            .unwrap(),
    );

    let txn = env.begin_rw_txn().unwrap();
    let db1 = txn.create_db(Some("t1"), DatabaseFlags::empty()).unwrap();
    let db2 = txn.create_db(Some("t2"), DatabaseFlags::empty()).unwrap();
    let db3 = txn.create_db(Some("t3"), DatabaseFlags::empty()).unwrap();
    let dbi1 = db1.dbi();
    let dbi2 = db2.dbi();
    let dbi3 = db3.dbi();
    txn.commit().unwrap();

    // Run many iterations to stress test concurrent commits
    for iteration in 0..20 {
        let txn = env.begin_rw_txn().unwrap();
        txn.enable_parallel_writes(&[dbi1, dbi2, dbi3]).unwrap();

        let txn = Arc::new(txn);

        let handles: Vec<_> = [dbi1, dbi2, dbi3]
            .iter()
            .map(|&dbi| {
                let txn = Arc::clone(&txn);
                thread::spawn(move || {
                    let mut cursor = txn.cursor_with_dbi_parallel(dbi).unwrap();
                    for i in 0..100u32 {
                        let key = format!("iter{iteration}_key{i}");
                        let val = format!("value_{dbi}_{i}");
                        cursor.put(key.as_bytes(), val.as_bytes(), WriteFlags::UPSERT).unwrap();
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        // All subtxns commit - mutex ensures no race
        // Use Arc::try_unwrap to get ownership back
        let mut txn = Arc::try_unwrap(txn).expect("All thread handles joined");
        txn.commit_subtxns().unwrap();
        txn.commit().unwrap();
    }

    // Verify final state
    let txn = env.begin_ro_txn().unwrap();
    for dbi in [dbi1, dbi2, dbi3] {
        let cursor = txn.cursor(dbi).unwrap();
        let count: usize = cursor.iter_slices().count();
        // 20 iterations * 100 keys = 2000, but UPSERT overwrites, so should have 100
        assert!(count >= 100, "Table {dbi} should have at least 100 entries, got {count}");
    }
}

// =============================================================================
// INVARIANT 7: Parent direct write blocked while subtxns active
// =============================================================================

#[test]
fn test_invariant_parent_put_blocked() {
    let dir = tempdir().unwrap();
    let env = Environment::builder()
        .set_max_dbs(10)
        .set_geometry(Geometry { size: Some(0..(1024 * 1024 * 10)), ..Default::default() })
        .write_map()
        .open(dir.path())
        .unwrap();

    let txn = env.begin_rw_txn().unwrap();
    let db = txn.create_db(Some("table"), DatabaseFlags::empty()).unwrap();
    let dbi = db.dbi();
    txn.commit().unwrap();

    let mut txn = env.begin_rw_txn().unwrap();
    txn.enable_parallel_writes(&[dbi]).unwrap();

    // Try to use parent's direct put - should fail
    let result = txn.put(dbi, b"key", b"value", WriteFlags::empty());
    assert!(result.is_err(), "Parent put should fail while subtxns active");

    txn.abort_subtxns().unwrap();
    drop(txn); // Abort via drop
}

// =============================================================================
// INVARIANT 8: Data visibility after commit
// =============================================================================

#[test]
fn test_invariant_data_visible_after_commit() {
    let dir = tempdir().unwrap();
    let env = Environment::builder()
        .set_max_dbs(10)
        .set_geometry(Geometry { size: Some(0..(1024 * 1024 * 10)), ..Default::default() })
        .write_map()
        .open(dir.path())
        .unwrap();

    let txn = env.begin_rw_txn().unwrap();
    let db = txn.create_db(Some("table"), DatabaseFlags::empty()).unwrap();
    let dbi = db.dbi();
    txn.commit().unwrap();

    // Write via parallel subtxn
    let mut txn = env.begin_rw_txn().unwrap();
    txn.enable_parallel_writes(&[dbi]).unwrap();

    {
        let mut cursor = txn.cursor_with_dbi_parallel(dbi).unwrap();
        cursor.put(b"key1", b"value1", WriteFlags::empty()).unwrap();
        cursor.put(b"key2", b"value2", WriteFlags::empty()).unwrap();
    }

    txn.commit_subtxns().unwrap();
    txn.commit().unwrap();

    // Verify via new read transaction
    let txn = env.begin_ro_txn().unwrap();
    let v1: Option<Cow<'_, [u8]>> = txn.get(dbi, b"key1").unwrap();
    let v2: Option<Cow<'_, [u8]>> = txn.get(dbi, b"key2").unwrap();

    assert_eq!(v1.as_deref(), Some(b"value1".as_slice()));
    assert_eq!(v2.as_deref(), Some(b"value2".as_slice()));
}

// =============================================================================
// INVARIANT 9: DUPSORT upsert behavior (documented in parallel-mdbx.md)
// =============================================================================

#[test]
fn test_invariant_dupsort_upsert_appends_not_replaces() {
    let dir = tempdir().unwrap();
    let env = Environment::builder()
        .set_max_dbs(10)
        .set_geometry(Geometry { size: Some(0..(1024 * 1024 * 10)), ..Default::default() })
        .write_map()
        .open(dir.path())
        .unwrap();

    let txn = env.begin_rw_txn().unwrap();
    let db = txn.create_db(Some("dupsort"), DatabaseFlags::DUP_SORT).unwrap();
    let dbi = db.dbi();
    txn.commit().unwrap();

    // Insert initial entry
    let txn = env.begin_rw_txn().unwrap();
    txn.put(dbi, b"key", b"value1", WriteFlags::empty()).unwrap();
    txn.commit().unwrap();

    // Use UPSERT with same key, different value
    let txn = env.begin_rw_txn().unwrap();
    txn.put(dbi, b"key", b"value2", WriteFlags::UPSERT).unwrap();
    txn.commit().unwrap();

    // In DUPSORT, UPSERT APPENDS, does not replace!
    let txn = env.begin_ro_txn().unwrap();
    let cursor = txn.cursor(dbi).unwrap();

    // Count all entries for the key using iter_slices
    let entries: Vec<_> = cursor.iter_slices().collect::<Result<Vec<_>>>().unwrap();

    // Filter for our key and collect values
    let key_entries: Vec<_> =
        entries.iter().filter(|(k, _)| k.as_ref() == b"key").map(|(_, v)| v.to_vec()).collect();

    // Should have BOTH values (appended, not replaced)
    assert_eq!(key_entries.len(), 2, "DUPSORT UPSERT should append, not replace");
    assert_eq!(key_entries[0], b"value1");
    assert_eq!(key_entries[1], b"value2");
}

// =============================================================================
// INVARIANT 10: Arena pages exhaustion triggers fallback
// =============================================================================

#[test]
fn test_invariant_arena_exhaustion_fallback() {
    let dir = tempdir().unwrap();
    let env = Environment::builder()
        .set_max_dbs(10)
        .set_geometry(Geometry { size: Some(0..(1024 * 1024 * 100)), ..Default::default() })
        .write_map()
        .open(dir.path())
        .unwrap();

    let txn = env.begin_rw_txn().unwrap();
    let db = txn.create_db(Some("table"), DatabaseFlags::empty()).unwrap();
    let dbi = db.dbi();
    txn.commit().unwrap();

    // Use very small arena hint - should trigger fallback
    let mut txn = env.begin_rw_txn().unwrap();
    txn.enable_parallel_writes_with_hints(&[(dbi, 2)]).unwrap(); // Only 2 pages!

    // Write enough data to exceed 2 pages
    {
        let mut cursor = txn.cursor_with_dbi_parallel(dbi).unwrap();
        for i in 0..1000u32 {
            let key = i.to_be_bytes();
            let val = [0xBB; 500]; // Large values to quickly exhaust pages
            cursor.put(&key, &val, WriteFlags::empty()).unwrap();
        }
    }

    // Should succeed via fallback to parent
    txn.commit_subtxns().unwrap();
    txn.commit().unwrap();

    // Verify data
    let txn = env.begin_ro_txn().unwrap();
    let cursor = txn.cursor(dbi).unwrap();
    let count: usize = cursor.iter_slices().count();
    assert_eq!(count, 1000);
}

// =============================================================================
// INVARIANT 11: Clone shares parallel_writes_enabled flag
// =============================================================================

#[test]
fn test_invariant_clone_shares_parallel_writes_flag() {
    // Clones of a transaction must share the parallel_writes_enabled flag.
    // When commit_subtxns() is called on one, all clones must see the change.
    let dir = tempdir().unwrap();
    let env = Environment::builder()
        .set_max_dbs(10)
        .set_geometry(Geometry { size: Some(0..(1024 * 1024 * 10)), ..Default::default() })
        .write_map()
        .open(dir.path())
        .unwrap();

    let txn = env.begin_rw_txn().unwrap();
    let db = txn.create_db(Some("table"), DatabaseFlags::empty()).unwrap();
    let dbi = db.dbi();
    txn.commit().unwrap();

    let mut txn = env.begin_rw_txn().unwrap();
    txn.enable_parallel_writes(&[dbi]).unwrap();

    // Clone the transaction
    let txn_clone = txn.clone();

    // Both should see parallel writes as enabled
    assert!(txn.is_parallel_writes_enabled());
    assert!(txn_clone.is_parallel_writes_enabled());

    // Write via the original transaction's subtxn
    {
        let mut cursor = txn.cursor_with_dbi_parallel(dbi).unwrap();
        cursor.put(b"key", b"value", WriteFlags::empty()).unwrap();
    }

    // Commit subtxns via the original - clone should also see the flag change
    txn.commit_subtxns().unwrap();

    // Both should now see parallel writes as disabled
    assert!(!txn.is_parallel_writes_enabled(), "Original should see flag as false");
    assert!(!txn_clone.is_parallel_writes_enabled(), "Clone must share flag state");

    // Now commit the parent (using either clone works since they share inner)
    txn.commit().unwrap();

    // Verify data persisted
    let txn = env.begin_ro_txn().unwrap();
    let val: Option<Cow<'_, [u8]>> = txn.get(dbi, b"key").unwrap();
    assert_eq!(val.as_deref(), Some(b"value".as_slice()));
}

// =============================================================================
// INVARIANT 12: Active cursor blocks commit_subtxns
// =============================================================================

#[test]
fn test_invariant_active_cursor_blocks_commit_subtxns() {
    let dir = tempdir().unwrap();
    let env = Environment::builder()
        .set_max_dbs(10)
        .set_geometry(Geometry { size: Some(0..(1024 * 1024 * 10)), ..Default::default() })
        .write_map()
        .open(dir.path())
        .unwrap();

    let txn = env.begin_rw_txn().unwrap();
    let db = txn.create_db(Some("table"), DatabaseFlags::empty()).unwrap();
    let dbi = db.dbi();
    txn.commit().unwrap();

    let txn = env.begin_rw_txn().unwrap();
    txn.enable_parallel_writes(&[dbi]).unwrap();

    // Get an owned cursor (keeps strong reference count elevated)
    let cursor = txn.cursor_with_dbi_parallel_owned(dbi).unwrap();

    // Attempt commit_subtxns() with active cursor - should return Error::Busy
    let result = txn.commit_subtxns();
    assert!(
        matches!(result, Err(Error::Busy)),
        "commit_subtxns should return Error::Busy with active cursor, got {:?}",
        result
    );

    // Drop the cursor
    drop(cursor);

    // Now commit_subtxns should succeed
    txn.commit_subtxns().expect("commit_subtxns should succeed after cursor dropped");
    txn.commit().expect("parent commit should succeed");
}

// =============================================================================
// INVARIANT 13: DBI opened in previous transaction handled correctly
// =============================================================================

#[test]
fn test_invariant_dbi_stale_handled() {
    let dir = tempdir().unwrap();
    let env = Environment::builder()
        .set_max_dbs(10)
        .set_geometry(Geometry { size: Some(0..(1024 * 1024 * 10)), ..Default::default() })
        .write_map()
        .open(dir.path())
        .unwrap();

    // Transaction 1: Open DBI and write some data
    let txn1 = env.begin_rw_txn().unwrap();
    let db = txn1.create_db(Some("table"), DatabaseFlags::empty()).unwrap();
    let dbi = db.dbi();
    txn1.put(dbi, b"initial_key", b"initial_value", WriteFlags::empty()).unwrap();
    txn1.commit().unwrap();

    // Transaction 2: Use the DBI from previous transaction with parallel writes
    let txn2 = env.begin_rw_txn().unwrap();
    // enable_parallel_writes should handle DBI correctly (pre-touch logic handles stale DBIs)
    txn2.enable_parallel_writes(&[dbi])
        .expect("enable_parallel_writes should handle DBI from previous txn");

    // Write data via parallel cursor
    {
        let mut cursor = txn2.cursor_with_dbi_parallel(dbi).unwrap();
        cursor.put(b"new_key", b"new_value", WriteFlags::empty()).unwrap();
    }

    // Commit subtxns and parent
    txn2.commit_subtxns().expect("commit_subtxns should succeed");
    txn2.commit().expect("parent commit should succeed");

    // Verify both initial and new data are readable
    let txn = env.begin_ro_txn().unwrap();
    let initial: Option<Cow<'_, [u8]>> = txn.get(dbi, b"initial_key").unwrap();
    let new: Option<Cow<'_, [u8]>> = txn.get(dbi, b"new_key").unwrap();
    assert_eq!(initial.as_deref(), Some(b"initial_value".as_slice()));
    assert_eq!(new.as_deref(), Some(b"new_value".as_slice()));
}

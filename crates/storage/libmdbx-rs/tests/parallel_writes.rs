#![allow(missing_docs)]
use reth_libmdbx::*;
use std::borrow::Cow;
use tempfile::tempdir;

#[test]
fn test_parallel_subtx_dupsort_storage_pattern() {
    // Setup with WRITEMAP mode
    let dir = tempdir().unwrap();
    let env = Environment::builder()
        .set_max_dbs(10)
        .set_geometry(Geometry { size: Some(0..(1024 * 1024 * 10)), ..Default::default() })
        .write_map()
        .open(dir.path())
        .unwrap();

    // Create DupSort db (like PlainStorageState)
    let txn = env.begin_rw_txn().unwrap();
    let db = txn.create_db(Some("storage"), DatabaseFlags::DUP_SORT).unwrap();
    let dbi = db.dbi();
    txn.commit().unwrap();

    // Insert initial data - address + storage_key as subkey pattern
    let txn = env.begin_rw_txn().unwrap();
    {
        let mut cursor = txn.cursor(dbi).unwrap();
        // addr1 has storage keys key1, key2
        cursor.put(b"addr1", b"key1\x00val1", WriteFlags::empty()).unwrap();
        cursor.put(b"addr1", b"key2\x00val2", WriteFlags::empty()).unwrap();
        // addr2 has storage key key1
        cursor.put(b"addr2", b"key1\x00val3", WriteFlags::empty()).unwrap();
    }
    txn.commit().unwrap();

    // Now do parallel subtxn with the exact write_state_changes pattern
    let txn = env.begin_rw_txn().unwrap();
    txn.enable_parallel_writes(&[dbi]).unwrap();

    {
        let mut cursor = txn.cursor_with_dbi_parallel(dbi).unwrap();

        // Simulate updating multiple storage entries like write_state_changes
        let updates: Vec<(&[u8], &[u8])> = vec![
            (b"addr1", b"key1\x00new_val1"), // update existing
            (b"addr1", b"key3\x00val_new"),  // insert new
            (b"addr2", b"key1\x00"),         // delete (zero value)
        ];

        for (addr, entry) in updates {
            let key_part = &entry[..4]; // first 4 bytes as "key"

            // Step 1: seek_by_key_subkey pattern using get_both_range
            let seek_result: Result<Option<Cow<'_, [u8]>>> = cursor.get_both_range(addr, key_part);

            if let Ok(Some(found_val)) = seek_result {
                // Check if the found key matches (like db_entry.key == entry.key)
                if found_val.starts_with(key_part) {
                    // Step 2: delete_current
                    cursor.del(WriteFlags::CURRENT).unwrap();
                }
            }

            // Step 3: upsert if value is not "zero" (not empty after key)
            if entry.len() > 5 {
                // has actual value
                cursor.put(addr, entry, WriteFlags::empty()).unwrap();
            }
        }
    }

    txn.commit_subtxns().unwrap();
    txn.commit().unwrap();

    // Verify
    let txn = env.begin_ro_txn().unwrap();
    let mut cursor = txn.cursor(dbi).unwrap();
    let entries: Vec<(Cow<'_, [u8]>, Cow<'_, [u8]>)> =
        cursor.iter_slices().collect::<Result<Vec<_>>>().unwrap();
    println!("Final entries: {} items", entries.len());
    for (k, v) in &entries {
        println!("  {:?} -> {:?}", String::from_utf8_lossy(k), String::from_utf8_lossy(v));
    }
    // Expected: addr1+key1 updated, addr1+key2 unchanged, addr1+key3 new, addr2+key1 deleted
    assert_eq!(entries.len(), 3);
}

#[test]
fn test_parallel_subtx_dupsort_realistic_data() {
    let dir = tempdir().unwrap();
    let env = Environment::builder()
        .set_max_dbs(10)
        .set_geometry(Geometry {
            size: Some(0..(1024 * 1024 * 100)), // 100MB
            ..Default::default()
        })
        .write_map()
        .open(dir.path())
        .unwrap();

    let txn = env.begin_rw_txn().unwrap();
    let db = txn.create_db(Some("realistic"), DatabaseFlags::DUP_SORT).unwrap();
    txn.commit().unwrap();

    // Create realistic data - 20 byte address as key, 64 byte storage entries as values
    let addr1: [u8; 20] = [0x11; 20];
    let addr2: [u8; 20] = [0x22; 20];

    // Storage entry: 32 byte key + 32 byte value
    let make_entry = |k: u8, v: u8| -> [u8; 64] {
        let mut entry = [0u8; 64];
        entry[..32].fill(k);
        entry[32..].fill(v);
        entry
    };

    // Insert initial data
    let txn = env.begin_rw_txn().unwrap();
    let dbi = db.dbi();
    {
        let mut cursor = txn.cursor(dbi).unwrap();
        cursor.put(&addr1, &make_entry(0x01, 0xAA), WriteFlags::empty()).unwrap();
        cursor.put(&addr1, &make_entry(0x02, 0xBB), WriteFlags::empty()).unwrap();
        cursor.put(&addr2, &make_entry(0x01, 0xCC), WriteFlags::empty()).unwrap();
    }
    txn.commit().unwrap();

    // Test parallel subtxn with realistic operations
    let txn = env.begin_rw_txn().unwrap();
    txn.enable_parallel_writes(&[dbi]).unwrap();

    {
        let mut cursor = txn.cursor_with_dbi_parallel(dbi).unwrap();

        // Update addr1's first entry - use get_both_range for DUPSORT seek
        let target = &make_entry(0x01, 0x00)[..32];
        let seek_result: Result<Option<Cow<'_, [u8]>>> = cursor.get_both_range(&addr1, target);
        if seek_result.is_ok() {
            cursor.del(WriteFlags::CURRENT).unwrap();
        }
        cursor.put(&addr1, &make_entry(0x01, 0xFF), WriteFlags::empty()).unwrap();
    }

    txn.commit_subtxns().unwrap();
    txn.commit().unwrap();

    println!("Realistic data test passed!");
}

/// Test that freelist pages are reused by parallel subtxns, not just EOF extension
#[test]
fn test_parallel_subtxn_freelist_reuse() {
    let dir = tempdir().unwrap();
    let env = Environment::builder()
        .set_max_dbs(10)
        .set_geometry(Geometry {
            size: Some(0..(1024 * 1024 * 100)), // 100MB
            ..Default::default()
        })
        .write_map()
        .open(dir.path())
        .unwrap();

    // Create multiple DBs like we have in reth
    let txn = env.begin_rw_txn().unwrap();
    let db1 = txn.create_db(Some("accounts"), DatabaseFlags::empty()).unwrap();
    let db2 = txn.create_db(Some("storage"), DatabaseFlags::DUP_SORT).unwrap();
    let db3 = txn.create_db(Some("trie"), DatabaseFlags::empty()).unwrap();
    let dbi1 = db1.dbi();
    let dbi2 = db2.dbi();
    let dbi3 = db3.dbi();
    txn.commit().unwrap();

    // Insert initial data to create pages that will be retired on update
    let txn = env.begin_rw_txn().unwrap();
    for i in 0..1000u32 {
        let key = i.to_be_bytes();
        let val = [0xAA; 100]; // ~100 bytes per entry
        txn.put(dbi1, &key, &val, WriteFlags::empty()).unwrap();
        txn.put(dbi3, &key, &val, WriteFlags::empty()).unwrap();
    }
    txn.commit().unwrap();

    // Get initial stat
    let stat_before = env.stat().unwrap();
    let freelist_before = env.freelist().unwrap();
    println!("Before updates: pages={}, freelist={}", stat_before.leaf_pages(), freelist_before);

    // Run multiple transactions with parallel subtxns
    // This should cause pages to be retired and then reused from freelist
    for round in 0..5 {
        let txn = env.begin_rw_txn().unwrap();
        
        // Enable parallel writes with arena hints
        txn.enable_parallel_writes_with_hints(&[
            (dbi1, 50),  // accounts - expect ~50 pages
            (dbi2, 20),  // storage - expect ~20 pages
            (dbi3, 30),  // trie - expect ~30 pages
        ]).unwrap();

        // Thread 1: Update accounts
        {
            let mut cursor = txn.cursor_with_dbi_parallel(dbi1).unwrap();
            for i in 0..200u32 {
                let key = i.to_be_bytes();
                let val = [round as u8; 100];
                cursor.put(&key, &val, WriteFlags::UPSERT).unwrap();
            }
        }

        // Thread 2: Update storage (simulated - same thread for simplicity)
        {
            let mut cursor = txn.cursor_with_dbi_parallel(dbi2).unwrap();
            for i in 0..100u32 {
                let key = i.to_be_bytes();
                let val = [round as u8; 64];
                cursor.put(&key, &val, WriteFlags::UPSERT).unwrap();
            }
        }

        // Thread 3: Update trie
        {
            let mut cursor = txn.cursor_with_dbi_parallel(dbi3).unwrap();
            for i in 0..150u32 {
                let key = i.to_be_bytes();
                let val = [round as u8; 80];
                cursor.put(&key, &val, WriteFlags::UPSERT).unwrap();
            }
        }

        txn.commit_subtxns().unwrap();
        txn.commit().unwrap();

        let freelist_after_round = env.freelist().unwrap();
        println!("After round {}: freelist={}", round, freelist_after_round);
    }

    let stat_after = env.stat().unwrap();
    let freelist_after = env.freelist().unwrap();
    println!("After all updates: pages={}, freelist={}", stat_after.leaf_pages(), freelist_after);

    // The freelist should NOT grow unboundedly
    // It may grow a bit due to B-tree rebalancing, but should stabilize
    // Allow 2x growth maximum - if it's more, pages aren't being reused
    let max_allowed_growth = freelist_before.saturating_mul(2).max(100);
    assert!(
        freelist_after <= max_allowed_growth,
        "Freelist grew too much: {} -> {} (max allowed: {}). Pages not being reused from GC!",
        freelist_before, freelist_after, max_allowed_growth
    );

    println!("Freelist reuse test passed! {} -> {}", freelist_before, freelist_after);
}

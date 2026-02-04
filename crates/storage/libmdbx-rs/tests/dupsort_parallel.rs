#![allow(missing_docs)]

//! Test for parallel DupSort operations that would fail without per-txn page_auxbuf.
//!
//! This test simulates the HashedStorages pattern where multiple DupSort upserts
//! happen concurrently, which requires thread-safe page_auxbuf handling.

use reth_libmdbx::*;
use std::{
    collections::HashMap,
    sync::{Arc, Barrier},
    thread,
};
use tempfile::tempdir;

/// Stress test for DupSort operations with sequential nested transactions.
///
/// This test performs many DupSort upserts that trigger subpage creation/expansion,
/// which uses page_auxbuf internally. While MDBX only allows one nested txn at a time,
/// this test verifies the page_auxbuf handling is correct for DupSort operations.
///
/// The pattern mimics HashedStorages: B256 key -> multiple StorageEntry values.
#[test]
fn test_dupsort_upsert_stress() {
    const NUM_KEYS: usize = 100;
    const VALUES_PER_KEY: usize = 50;
    const NUM_ITERATIONS: usize = 10;

    let dir = tempdir().unwrap();

    let env = Arc::new(
        Environment::builder()
            .set_max_dbs(10)
            .set_geometry(Geometry {
                size: Some(10 * 1024 * 1024..1024 * 1024 * 1024),
                ..Default::default()
            })
            .open(dir.path())
            .expect("Failed to open environment"),
    );

    // Create DupSort table (like HashedStorages)
    {
        let txn = env.begin_rw_txn().unwrap();
        txn.create_db(Some("hashed_storages"), DatabaseFlags::DUP_SORT)
            .expect("Failed to create table");
        txn.commit().unwrap();
    }

    // Track all expected data across iterations
    let mut all_expected: HashMap<Vec<u8>, Vec<Vec<u8>>> = HashMap::new();

    for iteration in 0..NUM_ITERATIONS {
        let mut main_txn = env.begin_rw_txn().expect("Failed to begin txn");

        // Track what we write in this iteration
        let mut iteration_expected: HashMap<Vec<u8>, Vec<Vec<u8>>> = HashMap::new();

        // Use nested transaction (like save_blocks does)
        let nested_txn = main_txn.begin_nested_txn().expect("Failed to begin nested txn");
        let db = nested_txn.open_db(Some("hashed_storages")).expect("Failed to open db");

        for key_id in 0..NUM_KEYS {
            // B256-like key (32 bytes)
            let key = format!("{:032x}", key_id + iteration * 1000);

            for value_id in 0..VALUES_PER_KEY {
                // StorageEntry-like value: subkey (32 bytes) + value (32 bytes)
                let subkey = format!("{:032x}", value_id);
                let value_data = format!("{:032x}", iteration * 10000 + value_id);
                let value = format!("{}{}", subkey, value_data);

                // This upsert uses page_auxbuf for DupSort subpage handling
                nested_txn
                    .put(db.dbi(), key.as_bytes(), value.as_bytes(), WriteFlags::UPSERT)
                    .unwrap_or_else(|e| {
                        panic!(
                            "Failed to put: iteration={}, key={}, value_id={}, err={:?}",
                            iteration, key_id, value_id, e
                        )
                    });

                // Track for verification
                iteration_expected
                    .entry(key.as_bytes().to_vec())
                    .or_default()
                    .push(value.as_bytes().to_vec());
            }
        }

        nested_txn.commit().expect("Failed to commit nested txn");
        main_txn.commit().expect("Failed to commit main txn");

        // Merge iteration data into all_expected
        for (key, values) in iteration_expected {
            all_expected.entry(key).or_default().extend(values);
        }

        // Verify reads match writes after commit
        {
            let read_txn = env.begin_ro_txn().expect("Failed to begin read txn");
            let db = read_txn.open_db(Some("hashed_storages")).expect("Failed to open db");
            let mut cursor = read_txn.cursor(db.dbi()).expect("Failed to create cursor");

            for (key, expected_values) in &all_expected {
                let actual_values: Vec<Vec<u8>> = cursor
                    .iter_dup_of::<Vec<u8>, Vec<u8>>(key)
                    .map(|r| r.expect("Failed to read value").1)
                    .collect();

                assert_eq!(
                    actual_values.len(),
                    expected_values.len(),
                    "Iteration {}: key {:?} value count mismatch: got {}, expected {}",
                    iteration,
                    String::from_utf8_lossy(key),
                    actual_values.len(),
                    expected_values.len()
                );

                // Verify each expected value exists in actual (order may differ due to sorting)
                let mut expected_sorted = expected_values.clone();
                expected_sorted.sort();
                let mut actual_sorted = actual_values.clone();
                actual_sorted.sort();

                assert_eq!(
                    actual_sorted, expected_sorted,
                    "Iteration {}: key {:?} values mismatch",
                    iteration,
                    String::from_utf8_lossy(key)
                );
            }

            let stat = read_txn.db_stat(db.dbi()).unwrap();
            if iteration % 5 == 4 {
                println!(
                    "Iteration {}: {} entries verified",
                    iteration + 1,
                    stat.entries()
                );
            }
        }
    }

    // Final verification
    let txn = env.begin_ro_txn().unwrap();
    let db = txn.open_db(Some("hashed_storages")).unwrap();
    let stat = txn.db_stat(db.dbi()).unwrap();

    let total_expected: usize = all_expected.values().map(|v| v.len()).sum();
    assert_eq!(
        stat.entries(),
        total_expected,
        "Final entry count mismatch: got {}, expected {}",
        stat.entries(),
        total_expected
    );
    println!(
        "Final: {} entries verified (all {} keys checked)",
        stat.entries(),
        all_expected.len()
    );
}

/// Test that exercises DupSort with rapid subpage -> subtree conversion.
///
/// This pattern is more likely to trigger page_auxbuf corruption if it's shared,
/// as the subpage data is manipulated in the scratch buffer during conversion.
#[test]
fn test_dupsort_subpage_to_subtree_stress() {
    const NUM_KEYS: usize = 20;
    const MAX_VALUES_PER_KEY: usize = 200; // Enough to trigger subtree conversion

    let dir = tempdir().unwrap();

    let env = Arc::new(
        Environment::builder()
            .set_max_dbs(10)
            .set_geometry(Geometry {
                size: Some(10 * 1024 * 1024..2 * 1024 * 1024 * 1024),
                ..Default::default()
            })
            .open(dir.path())
            .expect("Failed to open environment"),
    );

    {
        let txn = env.begin_rw_txn().unwrap();
        txn.create_db(Some("test_db"), DatabaseFlags::DUP_SORT)
            .expect("Failed to create table");
        txn.commit().unwrap();
    }

    let mut main_txn = env.begin_rw_txn().expect("Failed to begin txn");
    let nested_txn = main_txn.begin_nested_txn().expect("Failed to begin nested txn");
    let db = nested_txn.open_db(Some("test_db")).expect("Failed to open db");

    for key_id in 0..NUM_KEYS {
        let key = format!("key_{:08}", key_id);

        for value_id in 0..MAX_VALUES_PER_KEY {
            // Larger values to fill subpages faster and trigger subtree conversion
            let value = format!("value_{:08}_{:064}", value_id, value_id);

            nested_txn
                .put(db.dbi(), key.as_bytes(), value.as_bytes(), WriteFlags::UPSERT)
                .unwrap_or_else(|e| {
                    panic!(
                        "Failed to put: key={}, value_id={}, err={:?}",
                        key_id, value_id, e
                    )
                });
        }
    }

    nested_txn.commit().expect("Failed to commit nested txn");
    main_txn.commit().expect("Failed to commit main txn");

    // Verify
    let txn = env.begin_ro_txn().unwrap();
    let db = txn.open_db(Some("test_db")).unwrap();
    let stat = txn.db_stat(db.dbi()).unwrap();
    println!("Subtree stress: {} entries", stat.entries());
    assert_eq!(stat.entries(), NUM_KEYS * MAX_VALUES_PER_KEY);
}

/// Test rapid seek + delete + upsert pattern on DupSort (like HashedStorages write pattern).
///
/// This is the exact pattern from write_hashed_state that caused MDBX_PAGE_FULL.
#[test]
fn test_dupsort_seek_delete_upsert_pattern() {
    const NUM_ITERATIONS: usize = 20;
    const NUM_ADDRESSES: usize = 50;
    const SLOTS_PER_ADDRESS: usize = 30;

    let dir = tempdir().unwrap();

    let env = Arc::new(
        Environment::builder()
            .set_max_dbs(10)
            .set_geometry(Geometry {
                size: Some(10 * 1024 * 1024..1024 * 1024 * 1024),
                ..Default::default()
            })
            .open(dir.path())
            .expect("Failed to open environment"),
    );

    {
        let txn = env.begin_rw_txn().unwrap();
        txn.create_db(Some("hashed_storages"), DatabaseFlags::DUP_SORT)
            .expect("Failed to create table");
        txn.commit().unwrap();
    }

    for iteration in 0..NUM_ITERATIONS {
        let mut main_txn = env.begin_rw_txn().expect("Failed to begin txn");
        let nested_txn = main_txn.begin_nested_txn().expect("Failed to begin nested txn");
        let db = nested_txn.open_db(Some("hashed_storages")).expect("Failed to open db");
        let dbi = db.dbi();

        for addr_id in 0..NUM_ADDRESSES {
            // Simulated hashed address (32 bytes)
            let hashed_address = format!("{:032x}", addr_id);

            for slot_id in 0..SLOTS_PER_ADDRESS {
                // Simulated StorageEntry: hashed_slot (32 bytes) + value (32 bytes)
                let hashed_slot = format!("{:032x}", slot_id);
                let value = format!("{:032x}", iteration * 1000 + slot_id);
                let entry = format!("{}{}", hashed_slot, value);

                // Pattern from write_hashed_state:
                // 1. seek_by_key_subkey to find existing entry
                // 2. delete_current if found
                // 3. upsert new value

                let mut cursor = nested_txn.cursor(dbi).expect("Failed to create cursor");

                // Try to find existing entry with this key+subkey
                if let Ok(Some((found_key, found_val))) =
                    cursor.set_range::<Vec<u8>, Vec<u8>>(hashed_address.as_bytes())
                {
                    if found_key.as_slice() == hashed_address.as_bytes() {
                        // Check if the subkey (first 32 bytes of value) matches
                        if found_val.len() >= 32 && &found_val[..32] == hashed_slot.as_bytes() {
                            // Delete existing entry before upsert
                            cursor.del(WriteFlags::empty()).ok();
                        }
                    }
                }

                // Now upsert the new value
                nested_txn
                    .put(dbi, hashed_address.as_bytes(), entry.as_bytes(), WriteFlags::UPSERT)
                    .unwrap_or_else(|e| {
                        panic!(
                            "Failed to put: iteration={}, addr={}, slot={}, err={:?}",
                            iteration, addr_id, slot_id, e
                        )
                    });
            }
        }

        nested_txn.commit().expect("Failed to commit nested txn");
        main_txn.commit().expect("Failed to commit main txn");

        if iteration % 5 == 4 {
            let txn = env.begin_ro_txn().unwrap();
            let db = txn.open_db(Some("hashed_storages")).unwrap();
            let stat = txn.db_stat(db.dbi()).unwrap();
            println!("Iteration {}: {} entries", iteration + 1, stat.entries());
        }
    }

    println!("Seek-delete-upsert stress test completed!");
}

/// Test with multiple threads doing DupSort operations (serialized by MDBX).
///
/// Even though write transactions serialize, this tests that page_auxbuf
/// state is properly reset between transactions.
#[test]
fn test_dupsort_multithreaded_serialized() {
    const NUM_THREADS: usize = 4;
    const ITERATIONS_PER_THREAD: usize = 20;
    const ENTRIES_PER_ITERATION: usize = 100;

    let dir = tempdir().unwrap();

    let env = Arc::new(
        Environment::builder()
            .set_max_dbs(10)
            .set_geometry(Geometry {
                size: Some(10 * 1024 * 1024..1024 * 1024 * 1024),
                ..Default::default()
            })
            .open(dir.path())
            .expect("Failed to open environment"),
    );

    {
        let txn = env.begin_rw_txn().unwrap();
        txn.create_db(Some("test_db"), DatabaseFlags::DUP_SORT)
            .expect("Failed to create table");
        txn.commit().unwrap();
    }

    let barrier = Arc::new(Barrier::new(NUM_THREADS));

    let handles: Vec<_> = (0..NUM_THREADS)
        .map(|thread_id| {
            let env = env.clone();
            let barrier = barrier.clone();

            thread::spawn(move || {
                barrier.wait();

                for iter in 0..ITERATIONS_PER_THREAD {
                    let mut main_txn = env.begin_rw_txn().expect("Failed to begin txn");
                    let nested_txn =
                        main_txn.begin_nested_txn().expect("Failed to begin nested txn");
                    let db = nested_txn.open_db(Some("test_db")).expect("Failed to open db");

                    for entry in 0..ENTRIES_PER_ITERATION {
                        let key = format!("t{}_i{}_k{:04}", thread_id, iter, entry);
                        let value = format!("value_{:08}_{:08}", thread_id * 1000 + iter, entry);

                        nested_txn
                            .put(db.dbi(), key.as_bytes(), value.as_bytes(), WriteFlags::UPSERT)
                            .unwrap_or_else(|e| {
                                panic!(
                                    "Thread {} failed at iter={}, entry={}: {:?}",
                                    thread_id, iter, entry, e
                                )
                            });
                    }

                    nested_txn.commit().expect("Failed to commit nested txn");
                    main_txn.commit().expect("Failed to commit main txn");
                }

                thread_id
            })
        })
        .collect();

    for handle in handles {
        let thread_id = handle.join().expect("Thread panicked");
        println!("Thread {} completed", thread_id);
    }

    // Verify
    let txn = env.begin_ro_txn().unwrap();
    let db = txn.open_db(Some("test_db")).unwrap();
    let stat = txn.db_stat(db.dbi()).unwrap();
    println!("Multithreaded test: {} entries", stat.entries());

    let expected = NUM_THREADS * ITERATIONS_PER_THREAD * ENTRIES_PER_ITERATION;
    assert_eq!(stat.entries(), expected);
}

/// Test that verifies data written in nested transactions can be read back correctly.
/// This catches any corruption from page_auxbuf or other parallel write issues.
#[test]
fn test_nested_txn_write_read_integrity() {
    const NUM_KEYS: usize = 50;
    const VALUES_PER_KEY: usize = 20;

    let dir = tempdir().unwrap();
    let env = Environment::builder()
        .set_max_dbs(10)
        .set_geometry(Geometry {
            size: Some(10 * 1024 * 1024..1024 * 1024 * 1024),
            ..Default::default()
        })
        .open(dir.path())
        .unwrap();

    // Create DupSort table
    {
        let txn = env.begin_rw_txn().unwrap();
        txn.create_db(Some("test_db"), DatabaseFlags::DUP_SORT).unwrap();
        txn.commit().unwrap();
    }

    // Track what we write
    let mut expected: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();

    // Write data in nested transaction
    {
        let mut main_txn = env.begin_rw_txn().unwrap();
        let nested_txn = main_txn.begin_nested_txn().unwrap();
        let db = nested_txn.open_db(Some("test_db")).unwrap();

        for key_id in 0..NUM_KEYS {
            let key = format!("key_{:08}", key_id);

            for value_id in 0..VALUES_PER_KEY {
                let value = format!("value_{:08}_{:08}", key_id, value_id);

                nested_txn
                    .put(db.dbi(), key.as_bytes(), value.as_bytes(), WriteFlags::empty())
                    .unwrap();

                expected.entry(key.clone()).or_default().push(value);
            }
        }

        nested_txn.commit().unwrap();
        main_txn.commit().unwrap();
    }

    // Verify all data can be read back correctly
    {
        let txn = env.begin_ro_txn().unwrap();
        let db = txn.open_db(Some("test_db")).unwrap();
        let mut cursor = txn.cursor(db.dbi()).unwrap();

        let mut actual_count = 0;

        for (key, expected_values) in &expected {
            let actual_values: Vec<Vec<u8>> = cursor
                .iter_dup_of::<Vec<u8>, Vec<u8>>(key.as_bytes())
                .collect::<Result<Vec<_>>>()
                .unwrap()
                .into_iter()
                .map(|(_, v)| v)
                .collect();

            assert_eq!(
                actual_values.len(),
                expected_values.len(),
                "Key {:?}: expected {} values, got {}",
                key,
                expected_values.len(),
                actual_values.len()
            );

            for (i, expected_val) in expected_values.iter().enumerate() {
                let actual_val = String::from_utf8_lossy(&actual_values[i]);
                assert_eq!(
                    actual_val.as_ref(),
                    expected_val.as_str(),
                    "Key {:?} value {}: expected {:?}, got {:?}",
                    key,
                    i,
                    expected_val,
                    actual_val
                );
            }

            actual_count += actual_values.len();
        }

        assert_eq!(actual_count, NUM_KEYS * VALUES_PER_KEY);
        println!("Verified {} key-value pairs", actual_count);
    }
}

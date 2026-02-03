//! Tests for parallel write transactions (reth extension)
#![allow(missing_docs)]

use reth_mdbx_sys as ffi;
use std::{
    ffi::c_void,
    ptr,
    sync::{Arc, Barrier},
    thread,
};
use tempfile::tempdir;

/// Helper to convert a byte slice to MDBX_val
fn to_val(data: &[u8]) -> ffi::MDBX_val {
    ffi::MDBX_val { iov_base: data.as_ptr() as *mut c_void, iov_len: data.len() }
}

/// Basic test of the parallel subtx APIs with a single subtxn
#[test]
fn test_parallel_subtx_basic() {
    let dir = tempdir().unwrap();
    let path = std::ffi::CString::new(dir.path().to_str().unwrap()).unwrap();

    unsafe {
        // Create environment
        let mut env: *mut ffi::MDBX_env = ptr::null_mut();
        let rc = ffi::mdbx_env_create(&mut env);
        assert_eq!(rc, 0, "mdbx_env_create failed");

        let rc = ffi::mdbx_env_open(env, path.as_ptr(), ffi::MDBX_env_flags_t::default(), 0o644);
        assert_eq!(rc, 0, "mdbx_env_open failed");

        // Begin write transaction
        let mut txn: *mut ffi::MDBX_txn = ptr::null_mut();
        let rc = ffi::mdbx_txn_begin_ex(
            env,
            ptr::null_mut(),
            ffi::MDBX_TXN_READWRITE,
            &mut txn,
            ptr::null_mut(),
        );
        assert_eq!(rc, 0, "mdbx_txn_begin_ex failed");

        // Open default database
        let mut dbi: ffi::MDBX_dbi = 0;
        let rc = ffi::mdbx_dbi_open(txn, ptr::null(), ffi::MDBX_CREATE, &mut dbi);
        assert_eq!(rc, 0, "mdbx_dbi_open failed");

        // Reserve pages for parallel writes
        let mut range: ffi::MDBX_page_range_t = ffi::MDBX_page_range_t { begin: 0, end: 0 };
        let rc = ffi::mdbx_txn_reserve_pages(txn, 1000, &mut range);
        assert_eq!(rc, 0, "mdbx_txn_reserve_pages failed: {}", rc);
        assert!(range.end > range.begin, "page range should be non-empty");

        // Create a subtransaction
        let mut subtx: *mut ffi::MDBX_txn = ptr::null_mut();
        let rc = ffi::mdbx_txn_create_subtx(txn, &range, &mut subtx);
        assert_eq!(rc, 0, "mdbx_txn_create_subtx failed: {}", rc);
        assert!(!subtx.is_null());

        // Verify it's a subtxn
        assert_eq!(ffi::mdbx_txn_is_subtx(subtx), 1);
        assert_eq!(ffi::mdbx_txn_is_subtx(txn), 0);

        // Write data using subtx
        let key1 = b"key1";
        let val1 = b"value_from_subtx";
        let mut cursor: *mut ffi::MDBX_cursor = ptr::null_mut();
        let rc = ffi::mdbx_cursor_open(subtx, dbi, &mut cursor);
        assert_eq!(rc, 0, "mdbx_cursor_open failed: {}", rc);

        let mut k = to_val(key1);
        let mut v = to_val(val1);
        let rc = ffi::mdbx_cursor_put(cursor, &mut k, &mut v, ffi::MDBX_put_flags_t::default());
        assert_eq!(rc, 0, "mdbx_cursor_put failed: {}", rc);
        ffi::mdbx_cursor_close(cursor);

        // Commit subtransaction
        let rc = ffi::mdbx_subtx_commit(subtx);
        assert_eq!(rc, 0, "mdbx_subtx_commit failed: {}", rc);

        // Commit parent transaction
        let rc = ffi::mdbx_txn_commit_ex(txn, ptr::null_mut());
        assert_eq!(rc, 0, "mdbx_txn_commit failed: {}", rc);

        // Verify data was written by reading it back
        let mut txn: *mut ffi::MDBX_txn = ptr::null_mut();
        let rc = ffi::mdbx_txn_begin_ex(
            env,
            ptr::null_mut(),
            ffi::MDBX_TXN_RDONLY,
            &mut txn,
            ptr::null_mut(),
        );
        assert_eq!(rc, 0, "mdbx_txn_begin_ex for read failed");

        // Read key1
        let k = to_val(key1);
        let mut v = ffi::MDBX_val { iov_base: ptr::null_mut(), iov_len: 0 };
        let rc = ffi::mdbx_get(txn, dbi, &k, &mut v);
        assert_eq!(rc, 0, "mdbx_get key1 failed: {}", rc);
        let read_val = std::slice::from_raw_parts(v.iov_base as *const u8, v.iov_len);
        assert_eq!(read_val, val1, "key1 value mismatch");

        ffi::mdbx_txn_abort(txn);
        ffi::mdbx_env_close_ex(env, false);
    }
}

/// Test aborting a subtransaction
#[test]
fn test_parallel_subtx_abort() {
    let dir = tempdir().unwrap();
    let path = std::ffi::CString::new(dir.path().to_str().unwrap()).unwrap();

    unsafe {
        let mut env: *mut ffi::MDBX_env = ptr::null_mut();
        let rc = ffi::mdbx_env_create(&mut env);
        assert_eq!(rc, 0);

        let rc = ffi::mdbx_env_open(env, path.as_ptr(), ffi::MDBX_env_flags_t::default(), 0o644);
        assert_eq!(rc, 0);

        // Begin write transaction
        let mut txn: *mut ffi::MDBX_txn = ptr::null_mut();
        let rc = ffi::mdbx_txn_begin_ex(
            env,
            ptr::null_mut(),
            ffi::MDBX_TXN_READWRITE,
            &mut txn,
            ptr::null_mut(),
        );
        assert_eq!(rc, 0);

        let mut dbi: ffi::MDBX_dbi = 0;
        let rc = ffi::mdbx_dbi_open(txn, ptr::null(), ffi::MDBX_CREATE, &mut dbi);
        assert_eq!(rc, 0);

        // Reserve pages
        let mut range: ffi::MDBX_page_range_t = ffi::MDBX_page_range_t { begin: 0, end: 0 };
        let rc = ffi::mdbx_txn_reserve_pages(txn, 100, &mut range);
        assert_eq!(rc, 0);

        // Create subtx and write data
        let mut subtx: *mut ffi::MDBX_txn = ptr::null_mut();
        let rc = ffi::mdbx_txn_create_subtx(txn, &range, &mut subtx);
        assert_eq!(rc, 0);

        let key = b"abort_test_key";
        let val = b"this_should_not_exist";
        let mut cursor: *mut ffi::MDBX_cursor = ptr::null_mut();
        let rc = ffi::mdbx_cursor_open(subtx, dbi, &mut cursor);
        assert_eq!(rc, 0);

        let mut k = to_val(key);
        let mut v = to_val(val);
        let rc = ffi::mdbx_cursor_put(cursor, &mut k, &mut v, ffi::MDBX_put_flags_t::default());
        assert_eq!(rc, 0);
        ffi::mdbx_cursor_close(cursor);

        // Abort subtx instead of committing
        let rc = ffi::mdbx_subtx_abort(subtx);
        assert_eq!(rc, 0, "mdbx_subtx_abort failed: {}", rc);

        // Commit parent (data from aborted subtx should NOT be there)
        let rc = ffi::mdbx_txn_commit_ex(txn, ptr::null_mut());
        assert_eq!(rc, 0);

        // Verify the key does not exist
        let mut txn: *mut ffi::MDBX_txn = ptr::null_mut();
        let rc = ffi::mdbx_txn_begin_ex(
            env,
            ptr::null_mut(),
            ffi::MDBX_TXN_RDONLY,
            &mut txn,
            ptr::null_mut(),
        );
        assert_eq!(rc, 0);

        let k = to_val(key);
        let mut v = ffi::MDBX_val { iov_base: ptr::null_mut(), iov_len: 0 };
        let rc = ffi::mdbx_get(txn, dbi, &k, &mut v);
        assert_eq!(rc, ffi::MDBX_NOTFOUND, "aborted subtx data should not exist");

        ffi::mdbx_txn_abort(txn);
        ffi::mdbx_env_close_ex(env, false);
    }
}

/// Test sequential subtxn operations (each subtxn commits serially)
/// 
/// Note: True parallel writes to the same dbi are not supported because
/// B-tree modifications from concurrent subtxns cannot be merged. Each
/// subtxn must commit before the next one can safely write to the same dbi.
#[test]
fn test_parallel_subtx_threaded() {
    let dir = tempdir().unwrap();
    let path = std::ffi::CString::new(dir.path().to_str().unwrap()).unwrap();

    unsafe {
        let mut env: *mut ffi::MDBX_env = ptr::null_mut();
        ffi::mdbx_env_create(&mut env);
        ffi::mdbx_env_open(env, path.as_ptr(), ffi::MDBX_env_flags_t::default(), 0o644);

        // Begin write transaction
        let mut txn: *mut ffi::MDBX_txn = ptr::null_mut();
        ffi::mdbx_txn_begin_ex(env, ptr::null_mut(), ffi::MDBX_TXN_READWRITE, &mut txn, ptr::null_mut());

        let mut dbi: ffi::MDBX_dbi = 0;
        ffi::mdbx_dbi_open(txn, ptr::null(), ffi::MDBX_CREATE, &mut dbi);

        // Reserve a small number of pages to fit initial allocation
        let mut range: ffi::MDBX_page_range_t = ffi::MDBX_page_range_t { begin: 0, end: 0 };
        ffi::mdbx_txn_reserve_pages(txn, 30, &mut range);

        // Sequential subtxn operations (2 sequential subtxns)
        let num_subtxns = 2;
        let keys_per_subtxn = 1;
        
        for subtxn_id in 0..num_subtxns {
            let pages_per_subtxn = 10u64;
            let start = range.begin + (subtxn_id as u64) * pages_per_subtxn;
            let end = start + pages_per_subtxn;
            let worker_range = ffi::MDBX_page_range_t { begin: start, end };

            let mut subtx: *mut ffi::MDBX_txn = ptr::null_mut();
            let rc = ffi::mdbx_txn_create_subtx(txn, &worker_range, &mut subtx);
            assert_eq!(rc, 0, "create_subtx {subtxn_id} failed");

            // Write keys using this subtxn
            let mut cursor: *mut ffi::MDBX_cursor = ptr::null_mut();
            let rc = ffi::mdbx_cursor_open(subtx, dbi, &mut cursor);
            assert_eq!(rc, 0, "cursor_open {subtxn_id} failed");

            for i in 0..keys_per_subtxn {
                let key = format!("subtx{subtxn_id}_key{i:04}");
                let val = format!("value_from_subtx{subtxn_id}_{i}");

                let mut k = to_val(key.as_bytes());
                let mut v = to_val(val.as_bytes());
                let rc = ffi::mdbx_cursor_put(
                    cursor,
                    &mut k,
                    &mut v,
                    ffi::MDBX_put_flags_t::default(),
                );
                assert_eq!(rc, 0, "subtx {subtxn_id} put {i} failed: {rc}");
            }

            ffi::mdbx_cursor_close(cursor);

            // Commit this subtxn before creating the next one
            let rc = ffi::mdbx_subtx_commit(subtx);
            assert_eq!(rc, 0, "subtx {subtxn_id} commit failed: {rc}");
        }

        // Commit parent transaction
        let rc = ffi::mdbx_txn_commit_ex(txn, ptr::null_mut());
        assert_eq!(rc, 0, "parent txn commit failed");

        // Verify all keys were written
        let mut txn: *mut ffi::MDBX_txn = ptr::null_mut();
        ffi::mdbx_txn_begin_ex(env, ptr::null_mut(), ffi::MDBX_TXN_RDONLY, &mut txn, ptr::null_mut());

        for subtxn_id in 0..num_subtxns {
            for i in 0..keys_per_subtxn {
                let key = format!("subtx{subtxn_id}_key{i:04}");
                let k = to_val(key.as_bytes());
                let mut v = ffi::MDBX_val { iov_base: ptr::null_mut(), iov_len: 0 };
                let rc = ffi::mdbx_get(txn, dbi, &k, &mut v);
                assert_eq!(rc, 0, "missing key: {key}");
            }
        }

        ffi::mdbx_txn_abort(txn);
        ffi::mdbx_env_close_ex(env, false);
    }
}

/// Test true parallel writes to different DBIs from multiple threads
#[test]
fn test_parallel_subtx_different_dbis() {
    let dir = tempdir().unwrap();
    let path = std::ffi::CString::new(dir.path().to_str().unwrap()).unwrap();

    unsafe {
        let mut env: *mut ffi::MDBX_env = ptr::null_mut();
        ffi::mdbx_env_create(&mut env);
        ffi::mdbx_env_set_option(env, ffi::MDBX_opt_max_db, 4);
        ffi::mdbx_env_open(env, path.as_ptr(), ffi::MDBX_NOSTICKYTHREADS, 0o644);

        // Begin write transaction
        let mut txn: *mut ffi::MDBX_txn = ptr::null_mut();
        ffi::mdbx_txn_begin_ex(
            env,
            ptr::null_mut(),
            ffi::MDBX_TXN_READWRITE,
            &mut txn,
            ptr::null_mut(),
        );

        // Open separate databases for each worker
        let db_names: Vec<_> = (0..3)
            .map(|i| std::ffi::CString::new(format!("db{i}")).unwrap())
            .collect();
        let mut dbis: Vec<ffi::MDBX_dbi> = Vec::new();
        for name in &db_names {
            let mut dbi: ffi::MDBX_dbi = 0;
            let rc = ffi::mdbx_dbi_open(txn, name.as_ptr(), ffi::MDBX_CREATE, &mut dbi);
            assert_eq!(rc, 0, "mdbx_dbi_open failed");
            dbis.push(dbi);
        }

        // Reserve pages for parallel workers
        let num_workers = 3;
        let pages_per_worker = 100;
        let mut range: ffi::MDBX_page_range_t = ffi::MDBX_page_range_t { begin: 0, end: 0 };
        let rc = ffi::mdbx_txn_reserve_pages(txn, num_workers * pages_per_worker, &mut range);
        assert_eq!(rc, 0, "mdbx_txn_reserve_pages failed");

        // Create subtxns for each worker
        let mut subtxns: Vec<*mut ffi::MDBX_txn> = Vec::new();
        for i in 0..num_workers {
            let start = range.begin + (i as u64) * (pages_per_worker as u64);
            let end = start + pages_per_worker as u64;
            let worker_range = ffi::MDBX_page_range_t { begin: start, end };

            let mut subtx: *mut ffi::MDBX_txn = ptr::null_mut();
            let rc = ffi::mdbx_txn_create_subtx(txn, &worker_range, &mut subtx);
            assert_eq!(rc, 0, "mdbx_txn_create_subtx failed");
            subtxns.push(subtx);
        }

        // Each worker writes to its own DBI in parallel
        let keys_per_worker = 50;
        let barrier = Arc::new(Barrier::new(num_workers));

        let handles: Vec<_> = subtxns
            .into_iter()
            .enumerate()
            .map(|(worker_id, subtx)| {
                let barrier = barrier.clone();
                let subtx_ptr = subtx as usize;
                let dbi = dbis[worker_id];

                thread::spawn(move || {
                    barrier.wait();

                    let subtx = subtx_ptr as *mut ffi::MDBX_txn;
                    let mut cursor: *mut ffi::MDBX_cursor = ptr::null_mut();
                    let rc = ffi::mdbx_cursor_open(subtx, dbi, &mut cursor);
                    assert_eq!(rc, 0, "worker {worker_id} cursor_open failed: {rc}");

                    for i in 0..keys_per_worker {
                        let key = format!("worker{worker_id}_key{i:04}");
                        let val = format!("value_from_worker{worker_id}_{i}");

                        let mut k = to_val(key.as_bytes());
                        let mut v = to_val(val.as_bytes());
                        let rc = ffi::mdbx_cursor_put(
                            cursor,
                            &mut k,
                            &mut v,
                            ffi::MDBX_put_flags_t::default(),
                        );
                        assert_eq!(rc, 0, "worker {worker_id} put {i} failed: {rc}");
                    }

                    ffi::mdbx_cursor_close(cursor);
                    subtx_ptr
                })
            })
            .collect();

        // Collect results and commit subtxns serially
        // (commits must be serial even though writes were parallel)
        for (idx, h) in handles.into_iter().enumerate() {
            let subtx_ptr = h.join().expect("worker thread panicked");
            let subtx = subtx_ptr as *mut ffi::MDBX_txn;
            let rc = ffi::mdbx_subtx_commit(subtx);
            assert_eq!(rc, 0, "mdbx_subtx_commit {idx} failed: {rc}");
        }

        // Commit parent transaction
        let rc = ffi::mdbx_txn_commit_ex(txn, ptr::null_mut());
        assert_eq!(rc, 0, "parent txn commit failed");

        // Verify all keys were written
        let mut txn: *mut ffi::MDBX_txn = ptr::null_mut();
        ffi::mdbx_txn_begin_ex(
            env,
            ptr::null_mut(),
            ffi::MDBX_TXN_RDONLY,
            &mut txn,
            ptr::null_mut(),
        );

        for worker_id in 0..num_workers {
            let dbi = dbis[worker_id];
            for i in 0..keys_per_worker {
                let key = format!("worker{worker_id}_key{i:04}");
                let k = to_val(key.as_bytes());
                let mut v = ffi::MDBX_val { iov_base: ptr::null_mut(), iov_len: 0 };
                let rc = ffi::mdbx_get(txn, dbi, &k, &mut v);
                assert_eq!(rc, 0, "missing key in db{worker_id}: {key}");
            }
        }

        ffi::mdbx_txn_abort(txn);
        ffi::mdbx_env_close_ex(env, false);
    }
}

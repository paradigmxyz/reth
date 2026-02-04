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
        // Create environment with WRITEMAP (required for parallel subtxns)
        let mut env: *mut ffi::MDBX_env = ptr::null_mut();
        let rc = ffi::mdbx_env_create(&mut env);
        assert_eq!(rc, 0, "mdbx_env_create failed");

        ffi::mdbx_env_set_option(env, ffi::MDBX_opt_max_db, 4);
        let rc = ffi::mdbx_env_open(env, path.as_ptr(), ffi::MDBX_WRITEMAP, 0o644);
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

        // Open a named database
        let db_name = std::ffi::CString::new("testdb").unwrap();
        let mut dbi: ffi::MDBX_dbi = 0;
        let rc = ffi::mdbx_dbi_open(txn, db_name.as_ptr(), ffi::MDBX_CREATE, &mut dbi);
        assert_eq!(rc, 0, "mdbx_dbi_open failed");

        // Create a subtransaction using the new batch API
        let specs = [ffi::MDBX_subtxn_spec_t { dbi }];
        let mut subtxns: [*mut ffi::MDBX_txn; 1] = [ptr::null_mut()];
        let rc = ffi::mdbx_txn_create_subtxns(txn, specs.as_ptr(), 1, subtxns.as_mut_ptr());
        assert_eq!(rc, 0, "mdbx_txn_create_subtxns failed: {}", rc);

        let subtx = subtxns[0];
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

        ffi::mdbx_env_set_option(env, ffi::MDBX_opt_max_db, 4);
        let rc = ffi::mdbx_env_open(env, path.as_ptr(), ffi::MDBX_WRITEMAP, 0o644);
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

        let db_name = std::ffi::CString::new("abortdb").unwrap();
        let mut dbi: ffi::MDBX_dbi = 0;
        let rc = ffi::mdbx_dbi_open(txn, db_name.as_ptr(), ffi::MDBX_CREATE, &mut dbi);
        assert_eq!(rc, 0);

        // Create subtxn
        let specs = [ffi::MDBX_subtxn_spec_t { dbi }];
        let mut subtxns: [*mut ffi::MDBX_txn; 1] = [ptr::null_mut()];
        let rc = ffi::mdbx_txn_create_subtxns(txn, specs.as_ptr(), 1, subtxns.as_mut_ptr());
        assert_eq!(rc, 0);

        let subtx = subtxns[0];

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

/// Test parallel writes to two different DBIs
#[test]
fn test_parallel_subtx_two_dbis() {
    let dir = tempdir().unwrap();
    let path = std::ffi::CString::new(dir.path().to_str().unwrap()).unwrap();

    unsafe {
        let mut env: *mut ffi::MDBX_env = ptr::null_mut();
        ffi::mdbx_env_create(&mut env);
        ffi::mdbx_env_set_option(env, ffi::MDBX_opt_max_db, 4);
        ffi::mdbx_env_open(
            env,
            path.as_ptr(),
            ffi::MDBX_NOSTICKYTHREADS | ffi::MDBX_WRITEMAP,
            0o644,
        );

        // Begin write transaction
        let mut txn: *mut ffi::MDBX_txn = ptr::null_mut();
        ffi::mdbx_txn_begin_ex(
            env,
            ptr::null_mut(),
            ffi::MDBX_TXN_READWRITE,
            &mut txn,
            ptr::null_mut(),
        );

        // Open two separate databases
        let db0_name = std::ffi::CString::new("db0").unwrap();
        let db1_name = std::ffi::CString::new("db1").unwrap();

        let mut dbi0: ffi::MDBX_dbi = 0;
        let mut dbi1: ffi::MDBX_dbi = 0;
        ffi::mdbx_dbi_open(txn, db0_name.as_ptr(), ffi::MDBX_CREATE, &mut dbi0);
        ffi::mdbx_dbi_open(txn, db1_name.as_ptr(), ffi::MDBX_CREATE, &mut dbi1);

        // Create subtxns for both DBIs
        let specs = [ffi::MDBX_subtxn_spec_t { dbi: dbi0 }, ffi::MDBX_subtxn_spec_t { dbi: dbi1 }];
        let mut subtxns: [*mut ffi::MDBX_txn; 2] = [ptr::null_mut(); 2];
        let rc = ffi::mdbx_txn_create_subtxns(txn, specs.as_ptr(), 2, subtxns.as_mut_ptr());
        assert_eq!(rc, 0, "mdbx_txn_create_subtxns failed: {rc}");

        let keys_per_subtxn = 10;
        let barrier = Arc::new(Barrier::new(2));

        let handles: Vec<_> = subtxns
            .iter()
            .enumerate()
            .map(|(idx, &subtx)| {
                let barrier = barrier.clone();
                let subtx_ptr = subtx as usize;
                let dbi = if idx == 0 { dbi0 } else { dbi1 };

                thread::spawn(move || {
                    barrier.wait();

                    let subtx = subtx_ptr as *mut ffi::MDBX_txn;
                    let mut cursor: *mut ffi::MDBX_cursor = ptr::null_mut();
                    let rc = ffi::mdbx_cursor_open(subtx, dbi, &mut cursor);
                    assert_eq!(rc, 0, "cursor_open {idx} failed: {rc}");

                    for i in 0..keys_per_subtxn {
                        let key = format!("subtx{idx}_key{i:04}");
                        let val = format!("value_from_subtx{idx}_{i}");

                        let mut k = to_val(key.as_bytes());
                        let mut v = to_val(val.as_bytes());
                        let rc = ffi::mdbx_cursor_put(
                            cursor,
                            &mut k,
                            &mut v,
                            ffi::MDBX_put_flags_t::default(),
                        );
                        assert_eq!(rc, 0, "subtx {idx} put {i} failed: {rc}");
                    }

                    ffi::mdbx_cursor_close(cursor);
                    subtx_ptr
                })
            })
            .collect();

        // Commit subtxns serially
        for (idx, h) in handles.into_iter().enumerate() {
            let subtx_ptr = h.join().expect("worker panicked");
            let subtx = subtx_ptr as *mut ffi::MDBX_txn;
            let rc = ffi::mdbx_subtx_commit(subtx);
            assert_eq!(rc, 0, "mdbx_subtx_commit {idx} failed: {rc}");
        }

        // Commit parent
        let rc = ffi::mdbx_txn_commit_ex(txn, ptr::null_mut());
        assert_eq!(rc, 0, "parent commit failed");

        // Verify
        let mut txn: *mut ffi::MDBX_txn = ptr::null_mut();
        ffi::mdbx_txn_begin_ex(
            env,
            ptr::null_mut(),
            ffi::MDBX_TXN_RDONLY,
            &mut txn,
            ptr::null_mut(),
        );

        for idx in 0..2 {
            let dbi = if idx == 0 { dbi0 } else { dbi1 };
            for i in 0..keys_per_subtxn {
                let key = format!("subtx{idx}_key{i:04}");
                let k = to_val(key.as_bytes());
                let mut v = ffi::MDBX_val { iov_base: ptr::null_mut(), iov_len: 0 };
                let rc = ffi::mdbx_get(txn, dbi, &k, &mut v);
                assert_eq!(rc, 0, "missing key in db{idx}: {key}");
            }
        }

        ffi::mdbx_txn_abort(txn);
        ffi::mdbx_env_close_ex(env, false);
    }
}

/// Test parallel writes to different DBIs (3 workers)
#[test]
fn test_parallel_subtx_different_dbis() {
    let dir = tempdir().unwrap();
    let path = std::ffi::CString::new(dir.path().to_str().unwrap()).unwrap();

    unsafe {
        let mut env: *mut ffi::MDBX_env = ptr::null_mut();
        ffi::mdbx_env_create(&mut env);
        ffi::mdbx_env_set_option(env, ffi::MDBX_opt_max_db, 4);
        ffi::mdbx_env_open(
            env,
            path.as_ptr(),
            ffi::MDBX_NOSTICKYTHREADS | ffi::MDBX_WRITEMAP,
            0o644,
        );

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
        let db_names: Vec<_> =
            (0..3).map(|i| std::ffi::CString::new(format!("db{i}")).unwrap()).collect();
        let mut dbis: Vec<ffi::MDBX_dbi> = Vec::new();
        for name in &db_names {
            let mut dbi: ffi::MDBX_dbi = 0;
            let rc = ffi::mdbx_dbi_open(txn, name.as_ptr(), ffi::MDBX_CREATE, &mut dbi);
            assert_eq!(rc, 0, "mdbx_dbi_open failed");
            dbis.push(dbi);
        }

        // Create subtxns using new batch API
        let specs: Vec<_> = dbis.iter().map(|&dbi| ffi::MDBX_subtxn_spec_t { dbi }).collect();
        let mut subtxns: Vec<*mut ffi::MDBX_txn> = vec![ptr::null_mut(); 3];
        let rc = ffi::mdbx_txn_create_subtxns(txn, specs.as_ptr(), 3, subtxns.as_mut_ptr());
        assert_eq!(rc, 0, "mdbx_txn_create_subtxns failed: {rc}");

        // Each worker writes to its own DBI in parallel
        let num_workers = 3;
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

/// Test parallel writes to different DBIs with WRITEMAP mode (reth's default)
#[test]
fn test_parallel_subtx_writemap() {
    let dir = tempdir().unwrap();
    let path = std::ffi::CString::new(dir.path().to_str().unwrap()).unwrap();

    unsafe {
        let mut env: *mut ffi::MDBX_env = ptr::null_mut();
        ffi::mdbx_env_create(&mut env);
        ffi::mdbx_env_set_option(env, ffi::MDBX_opt_max_db, 4);
        ffi::mdbx_env_open(
            env,
            path.as_ptr(),
            ffi::MDBX_NOSTICKYTHREADS | ffi::MDBX_WRITEMAP,
            0o644,
        );

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
        let db_names: Vec<_> =
            (0..3).map(|i| std::ffi::CString::new(format!("db{i}")).unwrap()).collect();
        let mut dbis: Vec<ffi::MDBX_dbi> = Vec::new();
        for name in &db_names {
            let mut dbi: ffi::MDBX_dbi = 0;
            let rc = ffi::mdbx_dbi_open(txn, name.as_ptr(), ffi::MDBX_CREATE, &mut dbi);
            assert_eq!(rc, 0, "mdbx_dbi_open failed");
            dbis.push(dbi);
        }

        // Create subtxns using new batch API
        let specs: Vec<_> = dbis.iter().map(|&dbi| ffi::MDBX_subtxn_spec_t { dbi }).collect();
        let mut subtxns: Vec<*mut ffi::MDBX_txn> = vec![ptr::null_mut(); 3];
        let rc = ffi::mdbx_txn_create_subtxns(txn, specs.as_ptr(), 3, subtxns.as_mut_ptr());
        assert_eq!(rc, 0, "mdbx_txn_create_subtxns failed: {rc}");

        // Each worker writes to its own DBI in parallel
        let num_workers = 3;
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

/// Test that subtransactions can read pre-existing committed data
#[test]
fn test_parallel_subtx_with_preexisting_data() {
    let dir = tempdir().unwrap();
    let path = std::ffi::CString::new(dir.path().to_str().unwrap()).unwrap();

    unsafe {
        // Create environment with WRITEMAP
        let mut env: *mut ffi::MDBX_env = ptr::null_mut();
        let rc = ffi::mdbx_env_create(&mut env);
        assert_eq!(rc, 0, "mdbx_env_create failed");

        ffi::mdbx_env_set_option(env, ffi::MDBX_opt_max_db, 4);
        let rc = ffi::mdbx_env_open(env, path.as_ptr(), ffi::MDBX_WRITEMAP, 0o644);
        assert_eq!(rc, 0, "mdbx_env_open failed");

        // === PHASE 1: Pre-populate database ===
        let mut txn: *mut ffi::MDBX_txn = ptr::null_mut();
        let rc = ffi::mdbx_txn_begin_ex(
            env,
            ptr::null_mut(),
            ffi::MDBX_TXN_READWRITE,
            &mut txn,
            ptr::null_mut(),
        );
        assert_eq!(rc, 0, "phase1 txn_begin failed");

        let db_name = std::ffi::CString::new("testdb").unwrap();
        let mut dbi: ffi::MDBX_dbi = 0;
        let rc = ffi::mdbx_dbi_open(txn, db_name.as_ptr(), ffi::MDBX_CREATE, &mut dbi);
        assert_eq!(rc, 0, "mdbx_dbi_open failed");

        // Insert 10 keys
        for i in 0u64..10 {
            let key = i.to_be_bytes();
            let val = format!("preexisting_value_{i}");
            let mut k = to_val(&key);
            let mut v = to_val(val.as_bytes());
            let rc = ffi::mdbx_put(txn, dbi, &mut k, &mut v, ffi::MDBX_put_flags_t::default());
            assert_eq!(rc, 0, "phase1 put {i} failed: {rc}");
        }

        let rc = ffi::mdbx_txn_commit_ex(txn, ptr::null_mut());
        assert_eq!(rc, 0, "phase1 commit failed");
        println!("Phase 1: Pre-populated 10 keys and committed");

        // === PHASE 2: Create subtransaction and try to read + append ===
        let mut txn: *mut ffi::MDBX_txn = ptr::null_mut();
        let rc = ffi::mdbx_txn_begin_ex(
            env,
            ptr::null_mut(),
            ffi::MDBX_TXN_READWRITE,
            &mut txn,
            ptr::null_mut(),
        );
        assert_eq!(rc, 0, "phase2 txn_begin failed");

        // Verify parent can read existing data
        {
            let mut cursor: *mut ffi::MDBX_cursor = ptr::null_mut();
            let rc = ffi::mdbx_cursor_open(txn, dbi, &mut cursor);
            assert_eq!(rc, 0, "parent cursor_open failed");

            let mut k = ffi::MDBX_val { iov_base: ptr::null_mut(), iov_len: 0 };
            let mut v = ffi::MDBX_val { iov_base: ptr::null_mut(), iov_len: 0 };
            let rc = ffi::mdbx_cursor_get(cursor, &mut k, &mut v, ffi::MDBX_LAST);
            assert_eq!(rc, 0, "parent last() failed: {rc}");
            println!("Parent can read last key (len={})", k.iov_len);
            ffi::mdbx_cursor_close(cursor);
        }

        // Create subtransaction
        let specs = [ffi::MDBX_subtxn_spec_t { dbi }];
        let mut subtxns: [*mut ffi::MDBX_txn; 1] = [ptr::null_mut()];
        let rc = ffi::mdbx_txn_create_subtxns(txn, specs.as_ptr(), 1, subtxns.as_mut_ptr());
        assert_eq!(rc, 0, "mdbx_txn_create_subtxns failed: {rc}");
        let subtx = subtxns[0];
        println!("Created subtransaction");

        // Try to read existing data from subtransaction
        {
            let mut cursor: *mut ffi::MDBX_cursor = ptr::null_mut();
            let rc = ffi::mdbx_cursor_open(subtx, dbi, &mut cursor);
            assert_eq!(rc, 0, "subtx cursor_open failed: {rc}");

            let mut k = ffi::MDBX_val { iov_base: ptr::null_mut(), iov_len: 0 };
            let mut v = ffi::MDBX_val { iov_base: ptr::null_mut(), iov_len: 0 };
            let rc = ffi::mdbx_cursor_get(cursor, &mut k, &mut v, ffi::MDBX_LAST);
            println!("Subtx last() returned: {rc}");
            assert_eq!(rc, 0, "subtx last() failed: {rc}");
            ffi::mdbx_cursor_close(cursor);
        }

        // Commit subtransaction
        let rc = ffi::mdbx_subtx_commit(subtx);
        assert_eq!(rc, 0, "mdbx_subtx_commit failed: {rc}");

        // Commit parent
        let rc = ffi::mdbx_txn_commit_ex(txn, ptr::null_mut());
        assert_eq!(rc, 0, "parent commit failed");

        ffi::mdbx_env_close_ex(env, false);
    }
}

use reth_libmdbx::{DatabaseFlags, Environment, Geometry, WriteFlags};

/// Test parallel subtransactions with append operations and preexisting data.
/// This mirrors the real usage pattern in save_blocks which uses 18+ tables
/// with cursor.append() operations across multiple commit rounds.
#[test]
fn test_parallel_subtx_append_with_preexisting_data() {
    let dir = tempdir().unwrap();
    let env = Environment::builder()
        .set_max_dbs(20)
        .set_geometry(Geometry { size: Some(0..=(1024 * 1024 * 100)), ..Default::default() })
        .write_map()
        .open(dir.path())
        .unwrap();

    // Create multiple databases
    let dbis: Vec<_> = {
        let txn = env.begin_rw_txn().unwrap();
        let dbis: Vec<_> = (0..5)
            .map(|i| {
                let name = format!("table_{}", i);
                let db = txn.create_db(Some(&name), DatabaseFlags::empty()).unwrap();
                db.dbi()
            })
            .collect();
        txn.commit().unwrap();
        dbis
    };

    // First transaction: populate each table
    {
        let txn = env.begin_rw_txn().unwrap();
        for (i, &dbi) in dbis.iter().enumerate() {
            for j in 0..10u64 {
                let key = j.to_be_bytes();
                let val = format!("value_{}_{}", i, j);
                txn.put(dbi, &key, val.as_bytes(), WriteFlags::empty()).unwrap();
            }
        }
        txn.commit().unwrap();
    }

    // Second transaction: use subtransactions to append more data
    {
        let txn = env.begin_rw_txn().unwrap();

        // Enable parallel writes for all DBIs
        txn.enable_parallel_writes(&dbis).unwrap();

        // Try to append to each table using parallel cursors
        for (i, &dbi) in dbis.iter().enumerate() {
            let mut cursor = txn.cursor_with_dbi_parallel(dbi).unwrap();

            // First, find the last key
            let last = cursor.last::<Vec<u8>, Vec<u8>>().unwrap();
            println!("Table {}: last key = {:?}", i, last.map(|(k, _)| k));

            // Now try to append
            for j in 10..15u64 {
                let key = j.to_be_bytes();
                let val = format!("appended_{}_{}", i, j);
                // Use cursor put with APPEND flag
                cursor.put(&key, val.as_bytes(), WriteFlags::APPEND).unwrap();
            }
        }

        // Commit subtxns then parent
        txn.commit_subtxns().unwrap();
        txn.commit().unwrap();
    }

    // Verify all data
    {
        let txn = env.begin_ro_txn().unwrap();
        for (i, &dbi) in dbis.iter().enumerate() {
            let mut cursor = txn.cursor(dbi).unwrap();
            let count = cursor.iter::<Vec<u8>, Vec<u8>>().count();
            assert_eq!(count, 15, "Table {} should have 15 entries", i);
        }
        txn.commit().unwrap();
    }
}

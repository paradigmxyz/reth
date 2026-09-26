//! Adversarial regressions for the experimental parallel transaction API.

use reth_libmdbx::{DatabaseFlags, Environment, Error, Geometry, WriteFlags};
use std::{
    borrow::Cow,
    collections::BTreeMap,
    io::{BufRead, BufReader, Read, Write},
    process::{Command, Stdio},
    sync::Barrier,
    thread,
};

fn environment(path: &std::path::Path, upper: usize) -> Environment {
    Environment::builder()
        .set_max_dbs(16)
        .set_geometry(Geometry { size: Some(0..upper), ..Default::default() })
        .write_map()
        .open(path)
        .unwrap()
}

#[test]
fn parallel_iterators_use_the_child_transaction() {
    let dir = tempfile::tempdir().unwrap();
    let env = environment(dir.path(), 64 << 20);
    let tx = env.begin_rw_txn().unwrap();
    let dbi = tx.create_db(Some("rows"), DatabaseFlags::DUP_SORT).unwrap().dbi();
    tx.commit().unwrap();
    let tx = env.begin_rw_txn().unwrap();
    tx.enable_parallel_writes(&[dbi]).unwrap();
    tx.put_parallel(dbi, b"a", b"1", WriteFlags::empty()).unwrap();
    tx.put_parallel(dbi, b"a", b"2", WriteFlags::empty()).unwrap();
    let mut cursor = tx.cursor_with_dbi_parallel_owned(dbi).unwrap();
    let expected = vec![(b"a".to_vec(), b"1".to_vec()), (b"a".to_vec(), b"2".to_vec())];
    assert_eq!(
        cursor.iter_start::<Vec<u8>, Vec<u8>>().collect::<Result<Vec<_>, _>>().unwrap(),
        expected
    );
    assert_eq!(
        cursor.into_iter::<Vec<u8>, Vec<u8>>().collect::<Result<Vec<_>, _>>().unwrap(),
        Vec::new()
    );
    let cursor = tx.cursor_with_dbi_parallel_owned(dbi).unwrap();
    assert_eq!(
        cursor.into_iter::<Vec<u8>, Vec<u8>>().collect::<Result<Vec<_>, _>>().unwrap(),
        expected
    );
    tx.commit_subtxns().unwrap();
    tx.commit().unwrap();
}

#[test]
fn busy_abort_does_not_partially_abort_siblings() {
    for _ in 0..64 {
        let dir = tempfile::tempdir().unwrap();
        let env = environment(dir.path(), 64 << 20);
        let mut tx = env.begin_rw_txn().unwrap();
        let a = tx.create_db(Some("a"), DatabaseFlags::empty()).unwrap().dbi();
        let b = tx.create_db(Some("b"), DatabaseFlags::empty()).unwrap().dbi();
        tx.enable_parallel_writes(&[a, b]).unwrap();
        let cursor = tx.cursor_with_dbi_parallel_owned(a).unwrap();
        assert!(matches!(tx.abort_subtxns(), Err(Error::Busy)));
        tx.put_parallel(b, b"key", b"value", WriteFlags::empty())
            .expect("a Busy abort must leave every sibling usable");
        drop(cursor);
        tx.commit_subtxns().unwrap();
        tx.commit().unwrap();
    }
}

#[test]
fn committed_clone_cannot_modify_the_next_transaction() {
    let dir = tempfile::tempdir().unwrap();
    let env = environment(dir.path(), 64 << 20);
    let tx = env.begin_rw_txn().unwrap();
    let dbi = tx.create_db(Some("rows"), DatabaseFlags::empty()).unwrap().dbi();
    let stale = tx.clone();
    tx.commit().unwrap();
    let next = env.begin_rw_txn().unwrap();
    assert!(matches!(
        stale.put(dbi, b"intruder", b"value", WriteFlags::empty()),
        Err(Error::BadTxn)
    ));
    assert_eq!(next.get::<Vec<u8>>(dbi, b"intruder").unwrap(), None);
    next.commit().unwrap();
}

fn random(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

#[test]
fn randomized_parallel_writes_match_a_serial_model_and_reopen() {
    randomized_writes(true, false);
}

#[test]
fn randomized_serial_overflow_values_match_model_and_reopen() {
    randomized_writes(false, true);
}

#[test]
#[ignore = "known native allocator failure; run explicitly to reproduce"]
fn randomized_parallel_overflow_values_match_model_and_reopen() {
    randomized_writes(true, true);
}

fn randomized_writes(parallel: bool, overflow: bool) {
    for seed in 1..=8 {
        let dir = tempfile::tempdir().unwrap();
        let env = environment(dir.path(), 128 << 20);
        let setup = env.begin_rw_txn().unwrap();
        let dbis: Vec<_> = (0..4)
            .map(|i| setup.create_db(Some(&format!("t{i}")), DatabaseFlags::empty()).unwrap().dbi())
            .collect();
        setup.commit().unwrap();
        let mut model = vec![BTreeMap::new(); 4];
        for round in 0..32 {
            let before = model.clone();
            let pinned = env.begin_ro_txn().unwrap();
            let tx = env.begin_rw_txn().unwrap();
            if parallel {
                tx.enable_parallel_writes_with_hints(
                    &dbis.iter().map(|&d| (d, 1)).collect::<Vec<_>>(),
                )
                .unwrap();
            }
            let barrier = Barrier::new(4);
            thread::scope(|scope| {
                for (table, (&dbi, expected)) in dbis.iter().zip(&mut model).enumerate() {
                    let tx = &tx;
                    let barrier = &barrier;
                    scope.spawn(move || {
                        let mut rng = seed * 0x10001 + round * 0x101 + table as u64 + 1;
                        let mut cursor = if parallel {
                            tx.cursor_with_dbi_parallel_owned(dbi).unwrap()
                        } else {
                            tx.cursor(dbi).unwrap()
                        };
                        barrier.wait();
                        for _ in 0..256 {
                            let r = random(&mut rng);
                            let key = ((r >> 8) % 128).to_be_bytes().to_vec();
                            if r % 5 == 0 {
                                let actual = cursor.set::<Vec<u8>>(&key).unwrap();
                                assert_eq!(actual, expected.remove(&key));
                                if actual.is_some() {
                                    cursor.del(WriteFlags::CURRENT).unwrap();
                                }
                            } else {
                                // Include overflow pages and repeated size changes.
                                let sizes =
                                    if overflow { [1, 80, 1024, 8192] } else { [1, 32, 80, 256] };
                                let value = vec![(r >> 32) as u8; sizes[(r as usize >> 16) % 4]];
                                cursor.put(&key, &value, WriteFlags::empty()).unwrap_or_else(
                                    |error| {
                                        panic!(
                                            "seed {seed}, round {round}, table {table}: {error:?}"
                                        )
                                    },
                                );
                                expected.insert(key, value);
                            }
                        }
                    });
                }
            });
            if round % 7 == 0 {
                drop(tx);
                model = before.clone();
            } else {
                if parallel {
                    tx.commit_subtxns().unwrap();
                }
                tx.commit().unwrap();
            }
            let reader = env.begin_ro_txn().unwrap();
            for (i, &dbi) in dbis.iter().enumerate() {
                for (read, expected) in [(&pinned, &before[i]), (&reader, &model[i])] {
                    let rows = read
                        .cursor(dbi)
                        .unwrap()
                        .into_iter::<Vec<u8>, Vec<u8>>()
                        .collect::<Result<BTreeMap<_, _>, _>>()
                        .unwrap();
                    assert_eq!(&rows, expected, "seed {seed}, round {round}, table {i}");
                }
            }
        }
        drop(env);
        let reopened = environment(dir.path(), 128 << 20);
        let reader = reopened.begin_ro_txn().unwrap();
        for (i, expected) in model.iter().enumerate() {
            let dbi = reader.open_db(Some(&format!("t{i}"))).unwrap().dbi();
            let rows = reader
                .cursor(dbi)
                .unwrap()
                .into_iter::<Vec<u8>, Vec<u8>>()
                .collect::<Result<BTreeMap<_, _>, _>>()
                .unwrap();
            assert_eq!(&rows, expected, "reopen seed {seed}, table {i}");
        }
    }
}

#[test]
fn process_kill_recovers_an_atomic_generation() {
    for stage in ["during", "written", "merged", "committed"] {
        for _ in 0..8 {
            let dir = tempfile::tempdir().unwrap();
            let env = environment(dir.path(), 64 << 20);
            let tx = env.begin_rw_txn().unwrap();
            for i in 0..4 {
                let dbi =
                    tx.create_db(Some(&format!("t{i}")), DatabaseFlags::empty()).unwrap().dbi();
                for key in 0..256u32 {
                    tx.put(dbi, key.to_be_bytes(), [0; 96], WriteFlags::empty()).unwrap();
                }
            }
            tx.commit().unwrap();
            drop(env);
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "crash_checkpoint_child", "--ignored", "--nocapture"])
                .env("MDBX_AUDIT_CRASH_PATH", dir.path())
                .env("MDBX_AUDIT_CRASH_STAGE", stage)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
            let mut output = BufReader::new(child.stdout.take().unwrap());
            loop {
                let mut line = String::new();
                assert_ne!(output.read_line(&mut line).unwrap(), 0, "child exited before {stage}");
                if line.trim() == "READY" {
                    break;
                }
            }
            child.kill().unwrap();
            assert!(!child.wait().unwrap().success());
            let reopened = environment(dir.path(), 64 << 20);
            let read = reopened.begin_ro_txn().unwrap();
            for i in 0..4 {
                let dbi = read.open_db(Some(&format!("t{i}"))).unwrap().dbi();
                let rows = read
                    .cursor(dbi)
                    .unwrap()
                    .into_iter::<Vec<u8>, Vec<u8>>()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap();
                assert_eq!(rows.len(), 256);
                for (key, value) in rows {
                    assert_eq!(
                        value,
                        vec![u8::from(stage == "committed"); 96],
                        "stage {stage}, table {i}, key {key:?}"
                    );
                }
            }
        }
    }
}

#[test]
#[ignore = "subprocess entry point for process_kill_recovers_an_atomic_generation"]
fn crash_checkpoint_child() {
    let Ok(path) = std::env::var("MDBX_AUDIT_CRASH_PATH") else { return };
    let stage = std::env::var("MDBX_AUDIT_CRASH_STAGE").unwrap();
    let env = environment(std::path::Path::new(&path), 64 << 20);
    let tx = env.begin_rw_txn().unwrap();
    let dbis: Vec<_> = (0..4).map(|i| tx.open_db(Some(&format!("t{i}"))).unwrap().dbi()).collect();
    tx.enable_parallel_writes(&dbis).unwrap();
    thread::scope(|scope| {
        for (i, &dbi) in dbis.iter().enumerate() {
            let stage = &stage;
            let mut cursor = tx.cursor_with_dbi_parallel_owned(dbi).unwrap();
            scope.spawn(move || {
                for key in 0..256u32 {
                    cursor.put(&key.to_be_bytes(), &[1; 96], WriteFlags::empty()).unwrap();
                    if i == 0 && key == 128 && stage == "during" {
                        crash_checkpoint();
                    }
                }
            });
        }
    });
    if stage == "written" {
        crash_checkpoint();
    }
    tx.commit_subtxns().unwrap();
    if stage == "merged" {
        crash_checkpoint();
    }
    tx.commit().unwrap();
    if stage == "committed" {
        crash_checkpoint();
    }
}

fn crash_checkpoint() {
    println!("READY");
    std::io::stdout().flush().unwrap();
    let _ = std::io::stdin().read(&mut [0]);
    panic!("parent should kill the child at the checkpoint");
}

#[test]
fn concurrent_final_clones_abort_children_before_reusing_parent() {
    let dir = tempfile::tempdir().unwrap();
    let env = environment(dir.path(), 64 << 20);
    let setup = env.begin_rw_txn().unwrap();
    let dbi = setup.create_db(Some("rows"), DatabaseFlags::empty()).unwrap().dbi();
    setup.commit().unwrap();
    for _ in 0..128 {
        let tx = env.begin_rw_txn().unwrap();
        tx.enable_parallel_writes(&[dbi]).unwrap();
        tx.put_parallel(dbi, b"aborted", b"value", WriteFlags::empty()).unwrap();
        let other = tx.clone();
        let barrier = Barrier::new(2);
        thread::scope(|scope| {
            let barrier = &barrier;
            scope.spawn(move || {
                barrier.wait();
                drop(tx);
            });
            scope.spawn(move || {
                barrier.wait();
                drop(other);
            });
        });
        let next = env.begin_rw_txn().unwrap();
        assert_eq!(next.get::<Vec<u8>>(dbi, b"aborted").unwrap(), None);
        next.commit().unwrap();
    }
}

#[test]
fn cursor_creation_and_child_commit_are_mutually_exclusive() {
    let dir = tempfile::tempdir().unwrap();
    let env = environment(dir.path(), 64 << 20);
    let setup = env.begin_rw_txn().unwrap();
    let dbi = setup.create_db(Some("rows"), DatabaseFlags::empty()).unwrap().dbi();
    setup.commit().unwrap();
    for _ in 0..128 {
        let tx = env.begin_rw_txn().unwrap();
        tx.enable_parallel_writes(&[dbi]).unwrap();
        let barrier = Barrier::new(2);
        thread::scope(|scope| {
            scope.spawn(|| {
                barrier.wait();
                let mut cursor = tx.cursor_with_dbi_parallel_owned(dbi).unwrap();
                cursor.put(b"key", b"value", WriteFlags::empty()).unwrap();
            });
            scope.spawn(|| {
                barrier.wait();
                assert!(matches!(tx.commit_subtxns(), Ok(()) | Err(Error::Busy)));
            });
        });
        tx.commit_subtxns().unwrap();
        tx.commit().unwrap();
        let reader = env.begin_ro_txn().unwrap();
        assert_eq!(reader.get::<Vec<u8>>(dbi, b"key").unwrap(), Some(b"value".to_vec()));
    }
}

#[test]
fn parent_commit_with_live_cursor_is_recoverably_busy() {
    let dir = tempfile::tempdir().unwrap();
    let env = environment(dir.path(), 64 << 20);
    let tx = env.begin_rw_txn().unwrap();
    let dbi = tx.create_db(Some("rows"), DatabaseFlags::empty()).unwrap().dbi();
    let cursor = tx.cursor(dbi).unwrap();
    assert!(matches!(tx.clone().commit(), Err(Error::Busy)));
    drop(cursor);
    tx.commit().unwrap();
}

#[test]
#[ignore = "pre-existing borrowed-value lifetime hole; isolated crash reproducer"]
fn borrowed_result_outlives_closed_environment() {
    let value: Cow<'static, [u8]> = {
        let dir = tempfile::tempdir().unwrap();
        let env = environment(dir.path(), 64 << 20);
        let write = env.begin_rw_txn().unwrap();
        let dbi = write.create_db(Some("rows"), DatabaseFlags::empty()).unwrap().dbi();
        write.put(dbi, b"key", b"value", WriteFlags::empty()).unwrap();
        write.commit().unwrap();
        let read = env.begin_ro_txn().unwrap();
        read.get(dbi, b"key").unwrap().unwrap()
    };
    // This entirely safe program must not retain a borrow of an unmapped DB.
    assert_eq!(std::hint::black_box(value.as_ref()), b"value");
}

//! Adversarial regressions for the experimental parallel transaction API.

use reth_libmdbx::{DatabaseFlags, Environment, Error, Geometry, WriteFlags};
use std::{collections::BTreeMap, sync::Barrier, thread};

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
    assert_eq!(cursor.iter_start::<Vec<u8>, Vec<u8>>().collect::<Result<Vec<_>, _>>().unwrap(), expected);
    assert_eq!(cursor.into_iter::<Vec<u8>, Vec<u8>>().collect::<Result<Vec<_>, _>>().unwrap(), Vec::new());
    let cursor = tx.cursor_with_dbi_parallel_owned(dbi).unwrap();
    assert_eq!(cursor.into_iter::<Vec<u8>, Vec<u8>>().collect::<Result<Vec<_>, _>>().unwrap(), expected);
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
    assert!(matches!(stale.put(dbi, b"intruder", b"value", WriteFlags::empty()), Err(Error::BadTxn)));
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
            tx.enable_parallel_writes_with_hints(&dbis.iter().map(|&d| (d, 1)).collect::<Vec<_>>()).unwrap();
            let barrier = Barrier::new(4);
            thread::scope(|scope| {
                for (table, (&dbi, expected)) in dbis.iter().zip(&mut model).enumerate() {
                    let tx = &tx;
                    let barrier = &barrier;
                    scope.spawn(move || {
                        let mut rng = seed * 0x10001 + round * 0x101 + table as u64 + 1;
                        let mut cursor = tx.cursor_with_dbi_parallel_owned(dbi).unwrap();
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
                                let value = vec![(r >> 32) as u8; [1, 80, 1024, 8192][(r as usize >> 16) % 4]];
                                cursor.put(&key, &value, WriteFlags::empty()).unwrap();
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
                tx.commit_subtxns().unwrap();
                tx.commit().unwrap();
            }
            let reader = env.begin_ro_txn().unwrap();
            for (i, &dbi) in dbis.iter().enumerate() {
                for (read, expected) in [(&pinned, &before[i]), (&reader, &model[i])] {
                    let rows = read.cursor(dbi).unwrap().into_iter::<Vec<u8>, Vec<u8>>()
                        .collect::<Result<BTreeMap<_, _>, _>>().unwrap();
                    assert_eq!(&rows, expected, "seed {seed}, round {round}, table {i}");
                }
            }
        }
        drop(env);
        let reopened = environment(dir.path(), 128 << 20);
        let reader = reopened.begin_ro_txn().unwrap();
        for (i, expected) in model.iter().enumerate() {
            let dbi = reader.open_db(Some(&format!("t{i}"))).unwrap().dbi();
            let rows = reader.cursor(dbi).unwrap().into_iter::<Vec<u8>, Vec<u8>>()
                .collect::<Result<BTreeMap<_, _>, _>>().unwrap();
            assert_eq!(&rows, expected, "reopen seed {seed}, table {i}");
        }
    }
}

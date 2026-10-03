//! Regression checks for parallel child transaction lifetimes and persistence equivalence.
use reth_libmdbx::{DatabaseFlags, Environment, Error, Geometry, WriteFlags};

fn environment(path: &std::path::Path) -> Environment {
    Environment::builder()
        .set_max_dbs(8)
        .set_geometry(Geometry { size: Some(0..(64 * 1024 * 1024)), ..Default::default() })
        .write_map()
        .open(path)
        .unwrap()
}

#[test]
fn preparation_preserves_existing_max_key() {
    let dir = tempfile::tempdir().unwrap();
    let env = environment(dir.path());
    let tx = env.begin_rw_txn().unwrap();
    let dbi = tx.create_db(Some("data"), DatabaseFlags::empty()).unwrap().dbi();
    tx.put(dbi, [255; 8], b"existing", WriteFlags::empty()).unwrap();
    tx.commit().unwrap();
    let tx = env.begin_rw_txn().unwrap();
    tx.enable_parallel_writes(&[dbi]).unwrap();
    tx.commit_subtxns().unwrap();
    tx.commit().unwrap();
    let reader = env.begin_ro_txn().unwrap();
    assert_eq!(reader.get::<Vec<u8>>(dbi, &[255; 8]).unwrap().as_deref(), Some(&b"existing"[..]));
}

#[test]
fn live_cursors_block_finish_and_retry_preserves_writes() {
    let dir = tempfile::tempdir().unwrap();
    let env = environment(dir.path());
    let mut tx = env.begin_rw_txn().unwrap();
    let dbi = tx.create_db(Some("data"), DatabaseFlags::empty()).unwrap().dbi();
    let parent_cursor = tx.cursor(dbi).unwrap();
    assert!(matches!(tx.enable_parallel_writes(&[dbi]), Err(Error::Busy)));
    drop(parent_cursor);
    tx.enable_parallel_writes(&[dbi]).unwrap();
    let mut cursor = tx.cursor_with_dbi_parallel(dbi).unwrap();
    cursor.put(b"key", b"value", WriteFlags::empty()).unwrap();
    let clone = (*cursor).clone();
    assert!(matches!(tx.commit_subtxns(), Err(Error::Busy)));
    assert!(tx.is_parallel_writes_enabled());
    drop(cursor);
    assert!(matches!(tx.commit_subtxns_with_stats(), Err(Error::Busy)));
    assert!(matches!(tx.abort_subtxns(), Err(Error::Busy)));
    drop(clone);
    tx.commit_subtxns().unwrap();
    tx.commit().unwrap();
    let reader = env.begin_ro_txn().unwrap();
    assert_eq!(reader.get::<Vec<u8>>(dbi, b"key").unwrap().as_deref(), Some(&b"value"[..]));
}

#[test]
fn parallel_updates_match_serial_and_preserve_reader_snapshot() {
    let parallel_dir = tempfile::tempdir().unwrap();
    let serial_dir = tempfile::tempdir().unwrap();
    let parallel = environment(parallel_dir.path());
    let serial = environment(serial_dir.path());
    let mut parallel_dbis = Vec::new();
    let mut serial_dbis = Vec::new();
    for (env, dbis) in [(&parallel, &mut parallel_dbis), (&serial, &mut serial_dbis)] {
        let tx = env.begin_rw_txn().unwrap();
        for index in 0..4 {
            dbis.push(
                tx.create_db(Some(&format!("table{index}")), DatabaseFlags::empty()).unwrap().dbi(),
            );
        }
        tx.commit().unwrap();
    }
    for round in 0..24u32 {
        let snapshot = parallel.begin_ro_txn().unwrap();
        let tx = parallel.begin_rw_txn().unwrap();
        tx.enable_parallel_writes_with_hints(
            &parallel_dbis.iter().map(|&dbi| (dbi, 1)).collect::<Vec<_>>(),
        )
        .unwrap();
        std::thread::scope(|scope| {
            for &dbi in &parallel_dbis {
                let mut cursor = tx.cursor_with_dbi_parallel_owned(dbi).unwrap();
                scope.spawn(move || {
                    for key in 0..512u32 {
                        let key = key.to_be_bytes();
                        if round % 3 == 2 {
                            if cursor.set::<Vec<u8>>(&key).unwrap().is_some() {
                                cursor.del(WriteFlags::CURRENT).unwrap();
                            }
                        } else {
                            cursor
                                .put(
                                    &key,
                                    &vec![round as u8; 80 + round as usize * 8],
                                    WriteFlags::empty(),
                                )
                                .unwrap();
                        }
                    }
                });
            }
        });
        tx.commit_subtxns().unwrap();
        tx.commit().unwrap();
        let old_value = snapshot.get::<Vec<u8>>(parallel_dbis[0], &0u32.to_be_bytes()).unwrap();
        if round == 0 || (round - 1) % 3 == 2 {
            assert!(old_value.is_none());
        } else {
            assert_eq!(old_value.unwrap()[0], (round - 1) as u8);
        }
        drop(snapshot);
        let tx = serial.begin_rw_txn().unwrap();
        for &dbi in &serial_dbis {
            for key in 0..512u32 {
                if round % 3 == 2 {
                    tx.del(dbi, key.to_be_bytes(), None).unwrap();
                } else {
                    tx.put(
                        dbi,
                        key.to_be_bytes(),
                        vec![round as u8; 80 + round as usize * 8],
                        WriteFlags::empty(),
                    )
                    .unwrap();
                }
            }
        }
        tx.commit().unwrap();
        let parallel_reader = parallel.begin_ro_txn().unwrap();
        let serial_reader = serial.begin_ro_txn().unwrap();
        for (&p, &s) in parallel_dbis.iter().zip(&serial_dbis) {
            let actual = parallel_reader
                .cursor(p)
                .unwrap()
                .into_iter::<Vec<u8>, Vec<u8>>()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            let expected = serial_reader
                .cursor(s)
                .unwrap()
                .into_iter::<Vec<u8>, Vec<u8>>()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert_eq!(actual, expected, "round {round}, table {p}");
        }
    }
}

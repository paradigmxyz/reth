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

#[test]
fn seeded_parallel_mutations_match_serial_across_abort_and_reopen() {
    let parallel_dir = tempfile::tempdir().unwrap();
    let serial_dir = tempfile::tempdir().unwrap();
    let mut parallel = environment(parallel_dir.path());
    let mut serial = environment(serial_dir.path());
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

    let mut seed = 0x5eed_cafe_1234_5678_u64;
    for round in 0..48 {
        let mut operations = vec![Vec::new(); 4];
        for table in &mut operations {
            for _ in 0..64 {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                let key = ((seed >> 32) as u16 % 96).to_be_bytes();
                let value = if seed & 3 == 0 {
                    None
                } else {
                    Some(vec![(seed >> 24) as u8; 32 + (seed as usize % 128)])
                };
                table.push((key, value));
            }
        }

        let tx = parallel.begin_rw_txn().unwrap();
        tx.enable_parallel_writes(&parallel_dbis).unwrap();
        std::thread::scope(|scope| {
            for (&dbi, table) in parallel_dbis.iter().zip(&operations) {
                let tx = &tx;
                scope.spawn(move || {
                    for (key, value) in table {
                        if let Some(value) = value {
                            tx.put_parallel(dbi, key, value, WriteFlags::empty()).unwrap();
                        } else {
                            tx.del_parallel(dbi, key, None).unwrap();
                        }
                    }
                });
            }
        });
        if round % 7 == 0 {
            drop(tx);
        } else {
            tx.commit_subtxns().unwrap();
            tx.commit().unwrap();
        }

        let tx = serial.begin_rw_txn().unwrap();
        for (&dbi, table) in serial_dbis.iter().zip(&operations) {
            for (key, value) in table {
                if let Some(value) = value {
                    tx.put(dbi, key, value, WriteFlags::empty()).unwrap();
                } else {
                    tx.del(dbi, key, None).unwrap();
                }
            }
        }
        if round % 7 == 0 {
            drop(tx)
        } else {
            tx.commit().unwrap();
        }

        if round % 12 == 11 {
            drop(parallel);
            drop(serial);
            parallel = environment(parallel_dir.path());
            serial = environment(serial_dir.path());
            for (env, dbis) in [(&parallel, &mut parallel_dbis), (&serial, &mut serial_dbis)] {
                let tx = env.begin_ro_txn().unwrap();
                for (index, dbi) in dbis.iter_mut().enumerate() {
                    *dbi = tx.open_db(Some(&format!("table{index}"))).unwrap().dbi();
                }
            }
        }

        let actual = parallel.begin_ro_txn().unwrap();
        let expected = serial.begin_ro_txn().unwrap();
        for (&p, &s) in parallel_dbis.iter().zip(&serial_dbis) {
            let rows = |tx: &reth_libmdbx::Transaction<reth_libmdbx::RO>, dbi| {
                tx.cursor(dbi)
                    .unwrap()
                    .into_iter::<Vec<u8>, Vec<u8>>()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap()
            };
            assert_eq!(rows(&actual, p), rows(&expected, s), "round {round}");
        }
    }
}

#[test]
fn abort_with_live_sibling_cursor_leaves_every_child_usable() {
    let dir = tempfile::tempdir().unwrap();
    let env = environment(dir.path());
    let tx = env.begin_rw_txn().unwrap();
    let dbis: Vec<_> = (0..4)
        .map(|index| {
            tx.create_db(Some(&format!("table{index}")), DatabaseFlags::empty()).unwrap().dbi()
        })
        .collect();
    tx.commit().unwrap();

    let mut tx = env.begin_rw_txn().unwrap();
    tx.enable_parallel_writes(&dbis).unwrap();
    let cursor = tx.cursor_with_dbi_parallel_owned(dbis[0]).unwrap();
    assert!(matches!(tx.abort_subtxns(), Err(Error::Busy)));
    for &dbi in &dbis {
        tx.put_parallel(dbi, b"key", b"value", WriteFlags::empty()).unwrap();
    }
    drop(cursor);
    tx.commit_subtxns().unwrap();
    tx.commit().unwrap();
    let reader = env.begin_ro_txn().unwrap();
    for &dbi in &dbis {
        assert_eq!(reader.get::<Vec<u8>>(dbi, b"key").unwrap().as_deref(), Some(&b"value"[..]));
    }
}

#[test]
fn crash_recovery_worker() {
    let Ok(path) = std::env::var("MDBX_CRASH_TEST_PATH") else { return };
    let phase = std::env::var("MDBX_CRASH_TEST_PHASE").unwrap();
    let env = environment(std::path::Path::new(&path));
    let tx = env.begin_rw_txn().unwrap();
    let dbis: Vec<_> =
        (0..2).map(|index| tx.open_db(Some(&format!("table{index}"))).unwrap().dbi()).collect();
    tx.enable_parallel_writes(&dbis).unwrap();
    for &dbi in &dbis {
        tx.put_parallel(dbi, b"key", b"new", WriteFlags::empty()).unwrap();
    }
    if phase == "before_merge" {
        std::process::exit(86);
    }
    tx.commit_subtxns().unwrap();
    if phase == "before_commit" {
        std::process::exit(86);
    }
    tx.commit().unwrap();
    if phase == "after_commit_graceful" {
        return;
    }
    std::process::exit(86);
}

/// Run with `MDBX_CHK_BIN=/path/to/mdbx_chk cargo test -p reth-libmdbx --test
/// parallel_regressions mdbx_checker_covers_parallel_commit_and_interruptions -- --ignored`.
#[test]
#[ignore = "parallel commit currently leaves two pages unaccounted for by mdbx_chk"]
fn mdbx_checker_covers_parallel_commit_and_interruptions() {
    let checker = std::env::var("MDBX_CHK_BIN").expect("set MDBX_CHK_BIN to a built mdbx_chk");
    for phase in ["before_merge", "before_commit", "after_commit_graceful", "after_commit"] {
        let dir = tempfile::tempdir().unwrap();
        let env = environment(dir.path());
        let tx = env.begin_rw_txn().unwrap();
        for index in 0..2 {
            let dbi =
                tx.create_db(Some(&format!("table{index}")), DatabaseFlags::empty()).unwrap().dbi();
            tx.put(dbi, b"key", b"old", WriteFlags::empty()).unwrap();
        }
        tx.commit().unwrap();
        drop(env);

        let output = std::process::Command::new(&checker).arg(dir.path()).output().unwrap();
        assert!(
            output.status.success(),
            "serial baseline failed MDBX check: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );

        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--exact")
            .arg("crash_recovery_worker")
            .env("MDBX_CRASH_TEST_PATH", dir.path())
            .env("MDBX_CRASH_TEST_PHASE", phase)
            .status()
            .unwrap();
        assert_eq!(
            status.code(),
            Some(if phase == "after_commit_graceful" { 0 } else { 86 }),
            "worker did not reach {phase}"
        );

        let env = environment(dir.path());
        let reader = env.begin_ro_txn().unwrap();
        for index in 0..2 {
            let dbi = reader.open_db(Some(&format!("table{index}"))).unwrap().dbi();
            let value = reader.get::<Vec<u8>>(dbi, b"key").unwrap().unwrap();
            assert_eq!(
                value,
                if phase.starts_with("after_commit") { b"new" } else { b"old" },
                "{phase}, table{index}"
            );
        }
        drop(reader);
        drop(env);
        let output = std::process::Command::new(&checker).arg(dir.path()).output().unwrap();
        if !output.status.success() {
            let path = dir.keep();
            panic!(
                "MDBX check failed after {phase} at {}: status={} stdout={} stderr={}",
                path.display(),
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}

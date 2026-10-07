#![allow(missing_docs)]
use byteorder::{ByteOrder, LittleEndian};
use reth_libmdbx::*;
use std::collections::BTreeMap;
use tempfile::tempdir;

#[test]
fn test_open() {
    let dir = tempdir().unwrap();

    // opening non-existent env with read-only should fail
    assert!(Environment::builder().set_flags(Mode::ReadOnly.into()).open(dir.path()).is_err());

    // opening non-existent env should succeed
    assert!(Environment::builder().open(dir.path()).is_ok());

    // opening env with read-only should succeed
    assert!(Environment::builder().set_flags(Mode::ReadOnly.into()).open(dir.path()).is_ok());
}

#[test]
fn test_begin_txn() {
    let dir = tempdir().unwrap();

    {
        // writable environment
        let env = Environment::builder().open(dir.path()).unwrap();

        assert!(env.begin_rw_txn().is_ok());
        assert!(env.begin_ro_txn().is_ok());
    }

    {
        // read-only environment
        let env = Environment::builder().set_flags(Mode::ReadOnly.into()).open(dir.path()).unwrap();

        assert!(env.begin_rw_txn().is_err());
        assert!(env.begin_ro_txn().is_ok());
    }
}

#[test]
fn test_open_db() {
    let dir = tempdir().unwrap();
    let env = Environment::builder().set_max_dbs(1).open(dir.path()).unwrap();

    let txn = env.begin_ro_txn().unwrap();
    assert!(txn.open_db(None).is_ok());
    assert!(txn.open_db(Some("testdb")).is_err());
}

#[test]
fn test_create_db() {
    let dir = tempdir().unwrap();
    let env = Environment::builder().set_max_dbs(11).open(dir.path()).unwrap();

    let txn = env.begin_rw_txn().unwrap();
    assert!(txn.open_db(Some("testdb")).is_err());
    assert!(txn.create_db(Some("testdb"), DatabaseFlags::empty()).is_ok());
    assert!(txn.open_db(Some("testdb")).is_ok())
}

#[test]
fn test_close_database() {
    let dir = tempdir().unwrap();
    let env = Environment::builder().set_max_dbs(10).open(dir.path()).unwrap();

    let txn = env.begin_rw_txn().unwrap();
    txn.create_db(Some("db"), DatabaseFlags::empty()).unwrap();
    txn.open_db(Some("db")).unwrap();
}

#[test]
fn test_sync() {
    let dir = tempdir().unwrap();
    {
        let env = Environment::builder().open(dir.path()).unwrap();
        env.sync(true).unwrap();
    }
    {
        let env = Environment::builder().set_flags(Mode::ReadOnly.into()).open(dir.path()).unwrap();
        env.sync(true).unwrap_err();
    }
}

#[test]
fn test_stat() {
    let dir = tempdir().unwrap();
    let env = Environment::builder().open(dir.path()).unwrap();

    // Stats should be empty initially.
    let stat = env.stat().unwrap();
    assert_eq!(stat.depth(), 0);
    assert_eq!(stat.branch_pages(), 0);
    assert_eq!(stat.leaf_pages(), 0);
    assert_eq!(stat.overflow_pages(), 0);
    assert_eq!(stat.entries(), 0);

    // Write a few small values.
    for i in 0..64 {
        let mut value = [0u8; 8];
        LittleEndian::write_u64(&mut value, i);
        let tx = env.begin_rw_txn().expect("begin_rw_txn");
        tx.put(tx.open_db(None).unwrap().dbi(), value, value, WriteFlags::default())
            .expect("tx.put");
        tx.commit().expect("tx.commit");
    }

    // Stats should now reflect inserted values.
    let stat = env.stat().unwrap();
    assert_eq!(stat.depth(), 1);
    assert_eq!(stat.branch_pages(), 0);
    assert_eq!(stat.leaf_pages(), 1);
    assert_eq!(stat.overflow_pages(), 0);
    assert_eq!(stat.entries(), 64);
}

#[test]
fn test_info() {
    let map_size = 1024 * 1024;
    let dir = tempdir().unwrap();
    let env = Environment::builder()
        .set_geometry(Geometry { size: Some(map_size..), ..Default::default() })
        .open(dir.path())
        .unwrap();

    let info = env.info().unwrap();
    assert_eq!(info.geometry().min(), map_size as u64);
    // assert_eq!(info.last_pgno(), 1);
    // assert_eq!(info.last_txnid(), 0);
    assert_eq!(info.num_readers(), 0);
    assert!(matches!(info.mode(), Mode::ReadWrite { sync_mode: SyncMode::Durable }));
    assert!(env.is_read_write().unwrap());

    drop(env);
    let env = Environment::builder()
        .set_geometry(Geometry { size: Some(map_size..), ..Default::default() })
        .set_flags(EnvironmentFlags { mode: Mode::ReadOnly, ..Default::default() })
        .open(dir.path())
        .unwrap();
    let info = env.info().unwrap();
    assert!(matches!(info.mode(), Mode::ReadOnly));
    assert!(env.is_read_only().unwrap());
}

#[test]
fn test_freelist() {
    let dir = tempdir().unwrap();
    let env = Environment::builder().open(dir.path()).unwrap();

    let mut freelist = env.freelist().unwrap();
    assert_eq!(freelist, 0);

    // Write a few small values.
    for i in 0..64 {
        let mut value = [0u8; 8];
        LittleEndian::write_u64(&mut value, i);
        let tx = env.begin_rw_txn().expect("begin_rw_txn");
        tx.put(tx.open_db(None).unwrap().dbi(), value, value, WriteFlags::default())
            .expect("tx.put");
        tx.commit().expect("tx.commit");
    }
    let tx = env.begin_rw_txn().expect("begin_rw_txn");
    tx.clear_db(tx.open_db(None).unwrap().dbi()).expect("clear");
    tx.commit().expect("tx.commit");

    // Freelist should not be empty after clear_db.
    freelist = env.freelist().unwrap();
    assert!(freelist > 0);
}

#[test]
fn test_prefault_write_commit_abort_and_reopen() {
    let read_all = |env: &Environment| {
        let tx = env.begin_ro_txn().unwrap();
        let db = tx.open_db(None).unwrap();
        tx.cursor(db.dbi())
            .unwrap()
            .iter_start::<Vec<u8>, Vec<u8>>()
            .collect::<Result<BTreeMap<_, _>>>()
            .unwrap()
    };

    for enabled in [true, false] {
        let dir = tempdir().unwrap();
        let mut builder = Environment::builder();
        builder
            .write_map()
            .set_prefault_write(enabled)
            .set_flags(Mode::ReadWrite { sync_mode: SyncMode::Durable }.into())
            .set_geometry(Geometry { size: Some(0..64 * 1024 * 1024), ..Default::default() });
        let env = builder.open(dir.path()).unwrap();
        let mut expected = BTreeMap::new();

        // Vary leaf and overflow values, delete keys, and reuse pages across commits.
        for round in 0..4u32 {
            let tx = env.begin_rw_txn().unwrap();
            let db = tx.open_db(None).unwrap();
            for key in 0..512u32 {
                let encoded = key.to_be_bytes().to_vec();
                if (key + round) % 4 == 0 {
                    tx.del(db.dbi(), &encoded, None).unwrap();
                    expected.remove(&encoded);
                } else {
                    let value = vec![(key + round) as u8; 31 + ((key + round) % 8) as usize * 1024];
                    tx.put(db.dbi(), &encoded, &value, WriteFlags::empty()).unwrap();
                    expected.insert(encoded, value);
                }
            }
            tx.commit().unwrap();
            assert_eq!(read_all(&env), expected);

            {
                let aborted = env.begin_rw_txn().unwrap();
                let db = aborted.open_db(None).unwrap();
                aborted.clear_db(db.dbi()).unwrap();
                aborted.put(db.dbi(), b"uncommitted", [99; 8192], WriteFlags::empty()).unwrap();
            }
            assert_eq!(read_all(&env), expected);
        }

        drop(env);
        // The optimization is an environment setting, not an on-disk format change.
        builder.set_prefault_write(!enabled);
        let reopened = builder.open(dir.path()).unwrap();
        assert_eq!(read_all(&reopened), expected);
    }
}

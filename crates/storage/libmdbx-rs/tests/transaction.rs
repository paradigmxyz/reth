#![allow(missing_docs)]
use reth_libmdbx::*;
use std::{
    borrow::Cow,
    io::Write,
    sync::{Arc, Barrier},
    thread::{self, JoinHandle},
};
use tempfile::tempdir;

#[test]
fn test_put_get_del() {
    let dir = tempdir().unwrap();
    let env = Environment::builder().open(dir.path()).unwrap();

    let txn = env.begin_rw_txn().unwrap();
    let db = txn.open_db(None).unwrap();
    txn.put(db.dbi(), b"key1", b"val1", WriteFlags::empty()).unwrap();
    txn.put(db.dbi(), b"key2", b"val2", WriteFlags::empty()).unwrap();
    txn.put(db.dbi(), b"key3", b"val3", WriteFlags::empty()).unwrap();
    txn.commit().unwrap();

    let txn = env.begin_rw_txn().unwrap();
    let db = txn.open_db(None).unwrap();
    assert_eq!(txn.get(db.dbi(), b"key1").unwrap(), Some(*b"val1"));
    assert_eq!(txn.get(db.dbi(), b"key2").unwrap(), Some(*b"val2"));
    assert_eq!(txn.get(db.dbi(), b"key3").unwrap(), Some(*b"val3"));
    assert_eq!(txn.get::<()>(db.dbi(), b"key").unwrap(), None);

    txn.del(db.dbi(), b"key1", None).unwrap();
    assert_eq!(txn.get::<()>(db.dbi(), b"key1").unwrap(), None);
}

#[test]
fn test_put_get_del_multi() {
    let dir = tempdir().unwrap();
    let env = Environment::builder().open(dir.path()).unwrap();

    let txn = env.begin_rw_txn().unwrap();
    let db = txn.create_db(None, DatabaseFlags::DUP_SORT).unwrap();
    txn.put(db.dbi(), b"key1", b"val1", WriteFlags::empty()).unwrap();
    txn.put(db.dbi(), b"key1", b"val2", WriteFlags::empty()).unwrap();
    txn.put(db.dbi(), b"key1", b"val3", WriteFlags::empty()).unwrap();
    txn.put(db.dbi(), b"key2", b"val1", WriteFlags::empty()).unwrap();
    txn.put(db.dbi(), b"key2", b"val2", WriteFlags::empty()).unwrap();
    txn.put(db.dbi(), b"key2", b"val3", WriteFlags::empty()).unwrap();
    txn.put(db.dbi(), b"key3", b"val1", WriteFlags::empty()).unwrap();
    txn.put(db.dbi(), b"key3", b"val2", WriteFlags::empty()).unwrap();
    txn.put(db.dbi(), b"key3", b"val3", WriteFlags::empty()).unwrap();
    txn.commit().unwrap();

    let txn = env.begin_rw_txn().unwrap();
    let db = txn.open_db(None).unwrap();
    {
        let mut cur = txn.cursor(&db).unwrap();
        let iter = cur.iter_dup_of::<(), [u8; 4]>(b"key1");
        let vals = iter.map(|x| x.unwrap()).map(|(_, x)| x).collect::<Vec<_>>();
        assert_eq!(vals, vec![*b"val1", *b"val2", *b"val3"]);
    }
    txn.commit().unwrap();

    let txn = env.begin_rw_txn().unwrap();
    let db = txn.open_db(None).unwrap();
    txn.del(db.dbi(), b"key1", Some(b"val2")).unwrap();
    txn.del(db.dbi(), b"key2", None).unwrap();
    txn.commit().unwrap();

    let txn = env.begin_rw_txn().unwrap();
    let db = txn.open_db(None).unwrap();
    {
        let mut cur = txn.cursor(&db).unwrap();
        let iter = cur.iter_dup_of::<(), [u8; 4]>(b"key1");
        let vals = iter.map(|x| x.unwrap()).map(|(_, x)| x).collect::<Vec<_>>();
        assert_eq!(vals, vec![*b"val1", *b"val3"]);

        let iter = cur.iter_dup_of::<(), ()>(b"key2");
        assert_eq!(0, iter.count());
    }
    txn.commit().unwrap();
}

#[test]
fn test_put_get_del_empty_key() {
    let dir = tempdir().unwrap();
    let env = Environment::builder().open(dir.path()).unwrap();

    let txn = env.begin_rw_txn().unwrap();
    let db = txn.create_db(None, Default::default()).unwrap();
    txn.put(db.dbi(), b"", b"hello", WriteFlags::empty()).unwrap();
    assert_eq!(txn.get(db.dbi(), b"").unwrap(), Some(*b"hello"));
    txn.commit().unwrap();

    let txn = env.begin_rw_txn().unwrap();
    let db = txn.open_db(None).unwrap();
    assert_eq!(txn.get(db.dbi(), b"").unwrap(), Some(*b"hello"));
    txn.put(db.dbi(), b"", b"", WriteFlags::empty()).unwrap();
    assert_eq!(txn.get(db.dbi(), b"").unwrap(), Some(*b""));
}

#[test]
fn test_reserve() {
    let dir = tempdir().unwrap();
    let env = Environment::builder().open(dir.path()).unwrap();

    let txn = env.begin_rw_txn().unwrap();
    let db = txn.open_db(None).unwrap();
    {
        let mut writer = txn.reserve(&db, b"key1", 4, WriteFlags::empty()).unwrap();
        writer.write_all(b"val1").unwrap();
    }
    txn.commit().unwrap();

    let txn = env.begin_rw_txn().unwrap();
    let db = txn.open_db(None).unwrap();
    assert_eq!(txn.get(db.dbi(), b"key1").unwrap(), Some(*b"val1"));
    assert_eq!(txn.get::<()>(db.dbi(), b"key").unwrap(), None);

    txn.del(db.dbi(), b"key1", None).unwrap();
    assert_eq!(txn.get::<()>(db.dbi(), b"key1").unwrap(), None);
}

#[test]
fn test_nested_txn() {
    let dir = tempdir().unwrap();
    let env = Environment::builder().open(dir.path()).unwrap();

    let mut txn = env.begin_rw_txn().unwrap();
    txn.put(txn.open_db(None).unwrap().dbi(), b"key1", b"val1", WriteFlags::empty()).unwrap();

    {
        let nested = txn.begin_nested_txn().unwrap();
        let db = nested.open_db(None).unwrap();
        nested.put(db.dbi(), b"key2", b"val2", WriteFlags::empty()).unwrap();
        assert_eq!(nested.get(db.dbi(), b"key1").unwrap(), Some(*b"val1"));
        assert_eq!(nested.get(db.dbi(), b"key2").unwrap(), Some(*b"val2"));
    }

    let db = txn.open_db(None).unwrap();
    assert_eq!(txn.get(db.dbi(), b"key1").unwrap(), Some(*b"val1"));
    assert_eq!(txn.get::<()>(db.dbi(), b"key2").unwrap(), None);
}

#[test]
fn test_clear_db() {
    let dir = tempdir().unwrap();
    let env = Environment::builder().open(dir.path()).unwrap();

    {
        let txn = env.begin_rw_txn().unwrap();
        txn.put(txn.open_db(None).unwrap().dbi(), b"key", b"val", WriteFlags::empty()).unwrap();
        assert!(!txn.commit().unwrap().0);
    }

    {
        let txn = env.begin_rw_txn().unwrap();
        txn.clear_db(txn.open_db(None).unwrap().dbi()).unwrap();
        assert!(!txn.commit().unwrap().0);
    }

    let txn = env.begin_ro_txn().unwrap();
    assert_eq!(txn.get::<()>(txn.open_db(None).unwrap().dbi(), b"key").unwrap(), None);
}

#[test]
fn test_drop_db() {
    let dir = tempdir().unwrap();
    {
        let env = Environment::builder().set_max_dbs(2).open(dir.path()).unwrap();

        {
            let txn = env.begin_rw_txn().unwrap();
            txn.put(
                txn.create_db(Some("test"), DatabaseFlags::empty()).unwrap().dbi(),
                b"key",
                b"val",
                WriteFlags::empty(),
            )
            .unwrap();
            // Workaround for MDBX dbi drop issue
            txn.create_db(Some("canary"), DatabaseFlags::empty()).unwrap();
            assert!(!txn.commit().unwrap().0);
        }
        {
            let txn = env.begin_rw_txn().unwrap();
            let db = txn.open_db(Some("test")).unwrap();
            unsafe {
                txn.drop_db(db).unwrap();
            }
            assert!(matches!(txn.open_db(Some("test")).unwrap_err(), Error::NotFound));
            assert!(!txn.commit().unwrap().0);
        }
    }

    let env = Environment::builder().set_max_dbs(2).open(dir.path()).unwrap();

    let txn = env.begin_ro_txn().unwrap();
    txn.open_db(Some("canary")).unwrap();
    assert!(matches!(txn.open_db(Some("test")).unwrap_err(), Error::NotFound));
}

#[test]
fn test_concurrent_readers_single_writer() {
    let dir = tempdir().unwrap();
    let env: Arc<Environment> = Arc::new(Environment::builder().open(dir.path()).unwrap());

    let n = 10usize; // Number of concurrent readers
    let barrier = Arc::new(Barrier::new(n + 1));
    let mut threads: Vec<JoinHandle<bool>> = Vec::with_capacity(n);

    let key = b"key";
    let val = b"val";

    for _ in 0..n {
        let reader_env = env.clone();
        let reader_barrier = barrier.clone();

        threads.push(thread::spawn(move || {
            {
                let txn = reader_env.begin_ro_txn().unwrap();
                let db = txn.open_db(None).unwrap();
                assert_eq!(txn.get::<()>(db.dbi(), key).unwrap(), None);
            }
            reader_barrier.wait();
            reader_barrier.wait();
            {
                let txn = reader_env.begin_ro_txn().unwrap();
                let db = txn.open_db(None).unwrap();
                txn.get::<[u8; 3]>(db.dbi(), key).unwrap().unwrap() == *val
            }
        }));
    }

    let txn = env.begin_rw_txn().unwrap();
    let db = txn.open_db(None).unwrap();

    barrier.wait();
    txn.put(db.dbi(), key, val, WriteFlags::empty()).unwrap();
    txn.commit().unwrap();

    barrier.wait();

    assert!(threads.into_iter().all(|b| b.join().unwrap()))
}

#[test]
fn test_concurrent_writers() {
    let dir = tempdir().unwrap();
    let env = Arc::new(Environment::builder().open(dir.path()).unwrap());

    let n = 10usize; // Number of concurrent writers
    let mut threads: Vec<JoinHandle<bool>> = Vec::with_capacity(n);

    let key = "key";
    let val = "val";

    for i in 0..n {
        let writer_env = env.clone();

        threads.push(thread::spawn(move || {
            let txn = writer_env.begin_rw_txn().unwrap();
            let db = txn.open_db(None).unwrap();
            txn.put(db.dbi(), format!("{key}{i}"), format!("{val}{i}"), WriteFlags::empty())
                .unwrap();
            txn.commit().is_ok()
        }));
    }
    assert!(threads.into_iter().all(|b| b.join().unwrap()));

    let txn = env.begin_ro_txn().unwrap();
    let db = txn.open_db(None).unwrap();

    for i in 0..n {
        assert_eq!(
            Cow::<Vec<u8>>::Owned(format!("{val}{i}").into_bytes()),
            txn.get(db.dbi(), format!("{key}{i}").as_bytes()).unwrap().unwrap()
        );
    }
}

#[test]
fn test_stat() {
    let dir = tempdir().unwrap();
    let env = Environment::builder().open(dir.path()).unwrap();

    let txn = env.begin_rw_txn().unwrap();
    let db = txn.create_db(None, DatabaseFlags::empty()).unwrap();
    txn.put(db.dbi(), b"key1", b"val1", WriteFlags::empty()).unwrap();
    txn.put(db.dbi(), b"key2", b"val2", WriteFlags::empty()).unwrap();
    txn.put(db.dbi(), b"key3", b"val3", WriteFlags::empty()).unwrap();
    txn.commit().unwrap();

    {
        let txn = env.begin_ro_txn().unwrap();
        let db = txn.open_db(None).unwrap();
        let stat = txn.db_stat(&db).unwrap();
        assert_eq!(stat.entries(), 3);
    }

    let txn = env.begin_rw_txn().unwrap();
    let db = txn.open_db(None).unwrap();
    txn.del(db.dbi(), b"key1", None).unwrap();
    txn.del(db.dbi(), b"key2", None).unwrap();
    txn.commit().unwrap();

    {
        let txn = env.begin_ro_txn().unwrap();
        let db = txn.open_db(None).unwrap();
        let stat = txn.db_stat(&db).unwrap();
        assert_eq!(stat.entries(), 1);
    }

    let txn = env.begin_rw_txn().unwrap();
    let db = txn.open_db(None).unwrap();
    txn.put(db.dbi(), b"key4", b"val4", WriteFlags::empty()).unwrap();
    txn.put(db.dbi(), b"key5", b"val5", WriteFlags::empty()).unwrap();
    txn.put(db.dbi(), b"key6", b"val6", WriteFlags::empty()).unwrap();
    txn.commit().unwrap();

    {
        let txn = env.begin_ro_txn().unwrap();
        let db = txn.open_db(None).unwrap();
        let stat = txn.db_stat(&db).unwrap();
        assert_eq!(stat.entries(), 4);
    }
}

#[test]
fn test_stat_dupsort() {
    let dir = tempdir().unwrap();
    let env = Environment::builder().open(dir.path()).unwrap();

    let txn = env.begin_rw_txn().unwrap();
    let db = txn.create_db(None, DatabaseFlags::DUP_SORT).unwrap();
    txn.put(db.dbi(), b"key1", b"val1", WriteFlags::empty()).unwrap();
    txn.put(db.dbi(), b"key1", b"val2", WriteFlags::empty()).unwrap();
    txn.put(db.dbi(), b"key1", b"val3", WriteFlags::empty()).unwrap();
    txn.put(db.dbi(), b"key2", b"val1", WriteFlags::empty()).unwrap();
    txn.put(db.dbi(), b"key2", b"val2", WriteFlags::empty()).unwrap();
    txn.put(db.dbi(), b"key2", b"val3", WriteFlags::empty()).unwrap();
    txn.put(db.dbi(), b"key3", b"val1", WriteFlags::empty()).unwrap();
    txn.put(db.dbi(), b"key3", b"val2", WriteFlags::empty()).unwrap();
    txn.put(db.dbi(), b"key3", b"val3", WriteFlags::empty()).unwrap();
    txn.commit().unwrap();

    {
        let txn = env.begin_ro_txn().unwrap();
        let stat = txn.db_stat(&txn.open_db(None).unwrap()).unwrap();
        assert_eq!(stat.entries(), 9);
    }

    let txn = env.begin_rw_txn().unwrap();
    let db = txn.open_db(None).unwrap();
    txn.del(db.dbi(), b"key1", Some(b"val2")).unwrap();
    txn.del(db.dbi(), b"key2", None).unwrap();
    txn.commit().unwrap();

    {
        let txn = env.begin_ro_txn().unwrap();
        let stat = txn.db_stat(&txn.open_db(None).unwrap()).unwrap();
        assert_eq!(stat.entries(), 5);
    }

    let txn = env.begin_rw_txn().unwrap();
    let db = txn.open_db(None).unwrap();
    txn.put(db.dbi(), b"key4", b"val1", WriteFlags::empty()).unwrap();
    txn.put(db.dbi(), b"key4", b"val2", WriteFlags::empty()).unwrap();
    txn.put(db.dbi(), b"key4", b"val3", WriteFlags::empty()).unwrap();
    txn.commit().unwrap();

    {
        let txn = env.begin_ro_txn().unwrap();
        let stat = txn.db_stat(&txn.open_db(None).unwrap()).unwrap();
        assert_eq!(stat.entries(), 8);
    }
}

#[test]
fn test_clone_txn_same_snapshot() {
    let dir = tempdir().unwrap();
    let env = Environment::builder().open(dir.path()).unwrap();

    let txn = env.begin_rw_txn().unwrap();
    let db = txn.create_db(None, DatabaseFlags::empty()).unwrap();
    txn.put(db.dbi(), b"key1", b"val1", WriteFlags::empty()).unwrap();
    txn.put(db.dbi(), b"key2", b"val2", WriteFlags::empty()).unwrap();
    txn.commit().unwrap();

    let txn1 = env.begin_ro_txn().unwrap();
    let txn1_id = txn1.id().unwrap();

    let txn2 = txn1.clone_txn().unwrap();
    let txn2_id = txn2.id().unwrap();

    assert_eq!(txn1_id, txn2_id, "cloned txn should have same txnid");

    let db1 = txn1.open_db(None).unwrap();
    let db2 = txn2.open_db(None).unwrap();

    assert_eq!(txn1.get::<[u8; 4]>(db1.dbi(), b"key1").unwrap(), Some(*b"val1"));
    assert_eq!(txn2.get::<[u8; 4]>(db2.dbi(), b"key1").unwrap(), Some(*b"val1"));
    assert_eq!(txn1.get::<[u8; 4]>(db1.dbi(), b"key2").unwrap(), Some(*b"val2"));
    assert_eq!(txn2.get::<[u8; 4]>(db2.dbi(), b"key2").unwrap(), Some(*b"val2"));
}

#[test]
fn test_clone_txn_independent_lifetime() {
    let dir = tempdir().unwrap();
    let env = Environment::builder().open(dir.path()).unwrap();

    let txn = env.begin_rw_txn().unwrap();
    let db = txn.create_db(None, DatabaseFlags::empty()).unwrap();
    txn.put(db.dbi(), b"key1", b"val1", WriteFlags::empty()).unwrap();
    txn.commit().unwrap();

    let txn1 = env.begin_ro_txn().unwrap();
    let txn2 = txn1.clone_txn().unwrap();
    let txn2_id = txn2.id().unwrap();

    drop(txn1);

    let db2 = txn2.open_db(None).unwrap();
    assert_eq!(txn2.get::<[u8; 4]>(db2.dbi(), b"key1").unwrap(), Some(*b"val1"));
    assert_eq!(txn2.id().unwrap(), txn2_id);
}

#[test]
fn test_clone_txn_sees_same_snapshot_after_write() {
    let dir = tempdir().unwrap();
    let env = Environment::builder().open(dir.path()).unwrap();

    let txn = env.begin_rw_txn().unwrap();
    let db = txn.create_db(None, DatabaseFlags::empty()).unwrap();
    txn.put(db.dbi(), b"key1", b"val1", WriteFlags::empty()).unwrap();
    txn.commit().unwrap();

    let ro_txn = env.begin_ro_txn().unwrap();
    let cloned_txn = ro_txn.clone_txn().unwrap();

    {
        let write_txn = env.begin_rw_txn().unwrap();
        let db = write_txn.open_db(None).unwrap();
        write_txn.put(db.dbi(), b"key2", b"val2", WriteFlags::empty()).unwrap();
        write_txn.commit().unwrap();
    }

    let db1 = ro_txn.open_db(None).unwrap();
    let db2 = cloned_txn.open_db(None).unwrap();

    assert_eq!(ro_txn.get::<()>(db1.dbi(), b"key2").unwrap(), None);
    assert_eq!(cloned_txn.get::<()>(db2.dbi(), b"key2").unwrap(), None);

    let new_txn = env.begin_ro_txn().unwrap();
    let new_db = new_txn.open_db(None).unwrap();
    assert_eq!(new_txn.get::<[u8; 4]>(new_db.dbi(), b"key2").unwrap(), Some(*b"val2"));
}

#[test]
fn test_clone_txn_parallel_reads() {
    let dir = tempdir().unwrap();
    let env: Arc<Environment> = Arc::new(Environment::builder().open(dir.path()).unwrap());

    let txn = env.begin_rw_txn().unwrap();
    let db = txn.create_db(None, DatabaseFlags::empty()).unwrap();
    for i in 0..100 {
        txn.put(
            db.dbi(),
            format!("key{i:02}").as_bytes(),
            format!("val{i:02}").as_bytes(),
            WriteFlags::empty(),
        )
        .unwrap();
    }
    txn.commit().unwrap();

    let base_txn = env.begin_ro_txn().unwrap();
    let base_id = base_txn.id().unwrap();

    let n = 4usize;
    let barrier = Arc::new(Barrier::new(n));
    let mut handles: Vec<JoinHandle<(u64, usize)>> = Vec::with_capacity(n);

    for i in 0..n {
        let cloned_txn = base_txn.clone_txn().unwrap();
        let thread_barrier = barrier.clone();

        handles.push(thread::spawn(move || {
            thread_barrier.wait();

            let txn_id = cloned_txn.id().unwrap();
            let db = cloned_txn.open_db(None).unwrap();
            let mut count = 0usize;

            for j in (i * 25)..((i + 1) * 25) {
                let val: Cow<'_, [u8]> =
                    cloned_txn.get(db.dbi(), format!("key{j:02}").as_bytes()).unwrap().unwrap();
                assert!(val.starts_with(b"val"));
                count += 1;
            }

            (txn_id, count)
        }));
    }

    for handle in handles {
        let (txn_id, count) = handle.join().unwrap();
        assert_eq!(txn_id, base_id);
        assert_eq!(count, 25);
    }
}

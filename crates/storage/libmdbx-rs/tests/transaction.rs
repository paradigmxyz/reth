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
    let db_table = txn.open_table_inner(None).unwrap();
    txn.put(db_table.dbi(), b"key1", b"val1", WriteFlags::empty()).unwrap();
    txn.put(db_table.dbi(), b"key2", b"val2", WriteFlags::empty()).unwrap();
    txn.put(db_table.dbi(), b"key3", b"val3", WriteFlags::empty()).unwrap();
    txn.commit().unwrap();

    let txn = env.begin_rw_txn().unwrap();
    let db_table = txn.open_table_inner(None).unwrap();
    assert_eq!(txn.get(db_table.dbi(), b"key1").unwrap(), Some(*b"val1"));
    assert_eq!(txn.get(db_table.dbi(), b"key2").unwrap(), Some(*b"val2"));
    assert_eq!(txn.get(db_table.dbi(), b"key3").unwrap(), Some(*b"val3"));
    assert_eq!(txn.get::<()>(db_table.dbi(), b"key").unwrap(), None);

    txn.del(db_table.dbi(), b"key1", None).unwrap();
    assert_eq!(txn.get::<()>(db_table.dbi(), b"key1").unwrap(), None);
}

#[test]
fn test_put_get_del_multi() {
    let dir = tempdir().unwrap();
    let env = Environment::builder().open(dir.path()).unwrap();

    let txn = env.begin_rw_txn().unwrap();
    let db_table = txn.create_table_inner(None, TableFlags::DUP_SORT).unwrap();
    txn.put(db_table.dbi(), b"key1", b"val1", WriteFlags::empty()).unwrap();
    txn.put(db_table.dbi(), b"key1", b"val2", WriteFlags::empty()).unwrap();
    txn.put(db_table.dbi(), b"key1", b"val3", WriteFlags::empty()).unwrap();
    txn.put(db_table.dbi(), b"key2", b"val1", WriteFlags::empty()).unwrap();
    txn.put(db_table.dbi(), b"key2", b"val2", WriteFlags::empty()).unwrap();
    txn.put(db_table.dbi(), b"key2", b"val3", WriteFlags::empty()).unwrap();
    txn.put(db_table.dbi(), b"key3", b"val1", WriteFlags::empty()).unwrap();
    txn.put(db_table.dbi(), b"key3", b"val2", WriteFlags::empty()).unwrap();
    txn.put(db_table.dbi(), b"key3", b"val3", WriteFlags::empty()).unwrap();
    txn.commit().unwrap();

    let txn = env.begin_rw_txn().unwrap();
    let db_table = txn.open_table_inner(None).unwrap();
    {
        let mut cur = txn.cursor(&db_table).unwrap();
        let iter = cur.iter_dup_of::<(), [u8; 4]>(b"key1");
        let vals = iter.map(|x| x.unwrap()).map(|(_, x)| x).collect::<Vec<_>>();
        assert_eq!(vals, vec![*b"val1", *b"val2", *b"val3"]);
    }
    txn.commit().unwrap();

    let txn = env.begin_rw_txn().unwrap();
    let db_table = txn.open_table_inner(None).unwrap();
    txn.del(db_table.dbi(), b"key1", Some(b"val2")).unwrap();
    txn.del(db_table.dbi(), b"key2", None).unwrap();
    txn.commit().unwrap();

    let txn = env.begin_rw_txn().unwrap();
    let db_table = txn.open_table_inner(None).unwrap();
    {
        let mut cur = txn.cursor(&db_table).unwrap();
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
    let db_table = txn.create_table_inner(None, Default::default()).unwrap();
    txn.put(db_table.dbi(), b"", b"hello", WriteFlags::empty()).unwrap();
    assert_eq!(txn.get(db_table.dbi(), b"").unwrap(), Some(*b"hello"));
    txn.commit().unwrap();

    let txn = env.begin_rw_txn().unwrap();
    let db_table = txn.open_table_inner(None).unwrap();
    assert_eq!(txn.get(db_table.dbi(), b"").unwrap(), Some(*b"hello"));
    txn.put(db_table.dbi(), b"", b"", WriteFlags::empty()).unwrap();
    assert_eq!(txn.get(db_table.dbi(), b"").unwrap(), Some(*b""));
}

#[test]
fn test_reserve() {
    let dir = tempdir().unwrap();
    let env = Environment::builder().open(dir.path()).unwrap();

    let txn = env.begin_rw_txn().unwrap();
    let db_table = txn.open_table_inner(None).unwrap();
    {
        let mut writer = txn.reserve(&db_table, b"key1", 4, WriteFlags::empty()).unwrap();
        writer.write_all(b"val1").unwrap();
    }
    txn.commit().unwrap();

    let txn = env.begin_rw_txn().unwrap();
    let db_table = txn.open_table_inner(None).unwrap();
    assert_eq!(txn.get(db_table.dbi(), b"key1").unwrap(), Some(*b"val1"));
    assert_eq!(txn.get::<()>(db_table.dbi(), b"key").unwrap(), None);

    txn.del(db_table.dbi(), b"key1", None).unwrap();
    assert_eq!(txn.get::<()>(db_table.dbi(), b"key1").unwrap(), None);
}

#[test]
fn test_nested_txn() {
    let dir = tempdir().unwrap();
    let env = Environment::builder().open(dir.path()).unwrap();

    let mut txn = env.begin_rw_txn().unwrap();
    txn.put(txn.open_table_inner(None).unwrap().dbi(), b"key1", b"val1", WriteFlags::empty()).unwrap();

    {
        let nested = txn.begin_nested_txn().unwrap();
        let db_table = nested.open_table_inner(None).unwrap();
        nested.put(db_table.dbi(), b"key2", b"val2", WriteFlags::empty()).unwrap();
        assert_eq!(nested.get(db_table.dbi(), b"key1").unwrap(), Some(*b"val1"));
        assert_eq!(nested.get(db_table.dbi(), b"key2").unwrap(), Some(*b"val2"));
    }

    let db_table = txn.open_table_inner(None).unwrap();
    assert_eq!(txn.get(db_table.dbi(), b"key1").unwrap(), Some(*b"val1"));
    assert_eq!(txn.get::<()>(db_table.dbi(), b"key2").unwrap(), None);
}

#[test]
fn test_clear_db() {
    let dir = tempdir().unwrap();
    let env = Environment::builder().open(dir.path()).unwrap();

    {
        let txn = env.begin_rw_txn().unwrap();
        txn.put(txn.open_table_inner(None).unwrap().dbi(), b"key", b"val", WriteFlags::empty()).unwrap();
        assert!(!txn.commit().unwrap().0);
    }

    {
        let txn = env.begin_rw_txn().unwrap();
        txn.clear_table_inner(txn.open_table_inner(None).unwrap().dbi()).unwrap();
        assert!(!txn.commit().unwrap().0);
    }

    let txn = env.begin_ro_txn().unwrap();
    assert_eq!(txn.get::<()>(txn.open_table_inner(None).unwrap().dbi(), b"key").unwrap(), None);
}

#[test]
fn test_drop_db() {
    let dir = tempdir().unwrap();
    {
        let env = Environment::builder().set_max_dbs(2).open(dir.path()).unwrap();

        {
            let txn = env.begin_rw_txn().unwrap();
            txn.put(
                txn.create_table_inner(Some("test"), TableFlags::empty()).unwrap().dbi(),
                b"key",
                b"val",
                WriteFlags::empty(),
            )
            .unwrap();
            // Workaround for MDBX dbi drop issue
            txn.create_table_inner(Some("canary"), TableFlags::empty()).unwrap();
            assert!(!txn.commit().unwrap().0);
        }
        {
            let txn = env.begin_rw_txn().unwrap();
            let db_table = txn.open_table_inner(Some("test")).unwrap();
            unsafe {
                txn.drop_table_inner(db_table).unwrap();
            }
            assert!(matches!(txn.open_table_inner(Some("test")).unwrap_err(), Error::NotFound));
            assert!(!txn.commit().unwrap().0);
        }
    }

    let env = Environment::builder().set_max_dbs(2).open(dir.path()).unwrap();

    let txn = env.begin_ro_txn().unwrap();
    txn.open_table_inner(Some("canary")).unwrap();
    assert!(matches!(txn.open_table_inner(Some("test")).unwrap_err(), Error::NotFound));
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
                let db_table = txn.open_table_inner(None).unwrap();
                assert_eq!(txn.get::<()>(db_table.dbi(), key).unwrap(), None);
            }
            reader_barrier.wait();
            reader_barrier.wait();
            {
                let txn = reader_env.begin_ro_txn().unwrap();
                let db_table = txn.open_table_inner(None).unwrap();
                txn.get::<[u8; 3]>(db_table.dbi(), key).unwrap().unwrap() == *val
            }
        }));
    }

    let txn = env.begin_rw_txn().unwrap();
    let db_table = txn.open_table_inner(None).unwrap();

    barrier.wait();
    txn.put(db_table.dbi(), key, val, WriteFlags::empty()).unwrap();
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
            let db_table = txn.open_table_inner(None).unwrap();
            txn.put(db_table.dbi(), format!("{key}{i}"), format!("{val}{i}"), WriteFlags::empty())
                .unwrap();
            txn.commit().is_ok()
        }));
    }
    assert!(threads.into_iter().all(|b| b.join().unwrap()));

    let txn = env.begin_ro_txn().unwrap();
    let db_table = txn.open_table_inner(None).unwrap();

    for i in 0..n {
        assert_eq!(
            Cow::<Vec<u8>>::Owned(format!("{val}{i}").into_bytes()),
            txn.get(db_table.dbi(), format!("{key}{i}").as_bytes()).unwrap().unwrap()
        );
    }
}

#[test]
fn test_stat() {
    let dir = tempdir().unwrap();
    let env = Environment::builder().open(dir.path()).unwrap();

    let txn = env.begin_rw_txn().unwrap();
    let db_table = txn.create_table_inner(None, TableFlags::empty()).unwrap();
    txn.put(db_table.dbi(), b"key1", b"val1", WriteFlags::empty()).unwrap();
    txn.put(db_table.dbi(), b"key2", b"val2", WriteFlags::empty()).unwrap();
    txn.put(db_table.dbi(), b"key3", b"val3", WriteFlags::empty()).unwrap();
    txn.commit().unwrap();

    {
        let txn = env.begin_ro_txn().unwrap();
        let db_table = txn.open_table_inner(None).unwrap();
        let stat = txn.table_stat(&db_table).unwrap();
        assert_eq!(stat.entries(), 3);
    }

    let txn = env.begin_rw_txn().unwrap();
    let db_table = txn.open_table_inner(None).unwrap();
    txn.del(db_table.dbi(), b"key1", None).unwrap();
    txn.del(db_table.dbi(), b"key2", None).unwrap();
    txn.commit().unwrap();

    {
        let txn = env.begin_ro_txn().unwrap();
        let db_table = txn.open_table_inner(None).unwrap();
        let stat = txn.table_stat(&db_table).unwrap();
        assert_eq!(stat.entries(), 1);
    }

    let txn = env.begin_rw_txn().unwrap();
    let db_table = txn.open_table_inner(None).unwrap();
    txn.put(db_table.dbi(), b"key4", b"val4", WriteFlags::empty()).unwrap();
    txn.put(db_table.dbi(), b"key5", b"val5", WriteFlags::empty()).unwrap();
    txn.put(db_table.dbi(), b"key6", b"val6", WriteFlags::empty()).unwrap();
    txn.commit().unwrap();

    {
        let txn = env.begin_ro_txn().unwrap();
        let db_table = txn.open_table_inner(None).unwrap();
        let stat = txn.table_stat(&db_table).unwrap();
        assert_eq!(stat.entries(), 4);
    }
}

#[test]
fn test_stat_dupsort() {
    let dir = tempdir().unwrap();
    let env = Environment::builder().open(dir.path()).unwrap();

    let txn = env.begin_rw_txn().unwrap();
    let db_table = txn.create_table_inner(None, TableFlags::DUP_SORT).unwrap();
    txn.put(db_table.dbi(), b"key1", b"val1", WriteFlags::empty()).unwrap();
    txn.put(db_table.dbi(), b"key1", b"val2", WriteFlags::empty()).unwrap();
    txn.put(db_table.dbi(), b"key1", b"val3", WriteFlags::empty()).unwrap();
    txn.put(db_table.dbi(), b"key2", b"val1", WriteFlags::empty()).unwrap();
    txn.put(db_table.dbi(), b"key2", b"val2", WriteFlags::empty()).unwrap();
    txn.put(db_table.dbi(), b"key2", b"val3", WriteFlags::empty()).unwrap();
    txn.put(db_table.dbi(), b"key3", b"val1", WriteFlags::empty()).unwrap();
    txn.put(db_table.dbi(), b"key3", b"val2", WriteFlags::empty()).unwrap();
    txn.put(db_table.dbi(), b"key3", b"val3", WriteFlags::empty()).unwrap();
    txn.commit().unwrap();

    {
        let txn = env.begin_ro_txn().unwrap();
        let stat = txn.table_stat(&txn.open_table_inner(None).unwrap()).unwrap();
        assert_eq!(stat.entries(), 9);
    }

    let txn = env.begin_rw_txn().unwrap();
    let db_table = txn.open_table_inner(None).unwrap();
    txn.del(db_table.dbi(), b"key1", Some(b"val2")).unwrap();
    txn.del(db_table.dbi(), b"key2", None).unwrap();
    txn.commit().unwrap();

    {
        let txn = env.begin_ro_txn().unwrap();
        let stat = txn.table_stat(&txn.open_table_inner(None).unwrap()).unwrap();
        assert_eq!(stat.entries(), 5);
    }

    let txn = env.begin_rw_txn().unwrap();
    let db_table = txn.open_table_inner(None).unwrap();
    txn.put(db_table.dbi(), b"key4", b"val1", WriteFlags::empty()).unwrap();
    txn.put(db_table.dbi(), b"key4", b"val2", WriteFlags::empty()).unwrap();
    txn.put(db_table.dbi(), b"key4", b"val3", WriteFlags::empty()).unwrap();
    txn.commit().unwrap();

    {
        let txn = env.begin_ro_txn().unwrap();
        let stat = txn.table_stat(&txn.open_table_inner(None).unwrap()).unwrap();
        assert_eq!(stat.entries(), 8);
    }
}

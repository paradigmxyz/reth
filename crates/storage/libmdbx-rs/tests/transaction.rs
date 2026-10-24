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
    let dbi = txn.open_db(None).unwrap().dbi();
    {
        let mut cur = txn.cursor(dbi).unwrap();
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
    let dbi = txn.open_db(None).unwrap().dbi();
    {
        let mut cur = txn.cursor(dbi).unwrap();
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
    let dbi = txn.open_db(None).unwrap().dbi();
    {
        unsafe {
            // SAFETY: the returned slice is used before the transaction is committed or aborted.
            let mut writer = txn.reserve(dbi, b"key1", 4, WriteFlags::empty()).unwrap();
            writer.write_all(b"val1").unwrap();
        }
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
        txn.commit().unwrap();
    }

    {
        let txn = env.begin_rw_txn().unwrap();
        txn.clear_db(txn.open_db(None).unwrap().dbi()).unwrap();
        txn.commit().unwrap();
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
            txn.commit().unwrap();
        }
        {
            let txn = env.begin_rw_txn().unwrap();
            let dbi = txn.open_db(Some("test")).unwrap().dbi();
            unsafe {
                txn.drop_db(dbi).unwrap();
            }
            assert!(matches!(txn.open_db(Some("test")).unwrap_err(), Error::NotFound));
            txn.commit().unwrap();
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
        let dbi = txn.open_db(None).unwrap().dbi();
        let stat = txn.db_stat(dbi).unwrap();
        assert_eq!(stat.entries(), 3);
    }

    let txn = env.begin_rw_txn().unwrap();
    let db = txn.open_db(None).unwrap();
    txn.del(db.dbi(), b"key1", None).unwrap();
    txn.del(db.dbi(), b"key2", None).unwrap();
    txn.commit().unwrap();

    {
        let txn = env.begin_ro_txn().unwrap();
        let dbi = txn.open_db(None).unwrap().dbi();
        let stat = txn.db_stat(dbi).unwrap();
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
        let dbi = txn.open_db(None).unwrap().dbi();
        let stat = txn.db_stat(dbi).unwrap();
        assert_eq!(stat.entries(), 4);
    }
}

#[test]
fn test_stat_dupsort() {
    let dir = tempdir().unwrap();
    let env = Environment::builder().open(dir.path()).unwrap();

    let txn = env.begin_rw_txn().unwrap();
    let db = txn.create_db(None, DatabaseFlags::DUP_SORT).unwrap();
    let dbi = db.dbi();
    txn.put(dbi, b"key1", b"val1", WriteFlags::empty()).unwrap();
    txn.put(dbi, b"key1", b"val2", WriteFlags::empty()).unwrap();
    txn.put(dbi, b"key1", b"val3", WriteFlags::empty()).unwrap();
    txn.put(dbi, b"key2", b"val1", WriteFlags::empty()).unwrap();
    txn.put(dbi, b"key2", b"val2", WriteFlags::empty()).unwrap();
    txn.put(dbi, b"key2", b"val3", WriteFlags::empty()).unwrap();
    txn.put(dbi, b"key3", b"val1", WriteFlags::empty()).unwrap();
    txn.put(dbi, b"key3", b"val2", WriteFlags::empty()).unwrap();
    txn.put(dbi, b"key3", b"val3", WriteFlags::empty()).unwrap();
    txn.commit().unwrap();

    {
        let txn = env.begin_ro_txn().unwrap();
        let dbi = txn.open_db(None).unwrap().dbi();
        let stat = txn.db_stat(dbi).unwrap();
        assert_eq!(stat.entries(), 9);
    }

    let txn = env.begin_rw_txn().unwrap();
    let db = txn.open_db(None).unwrap();
    txn.del(db.dbi(), b"key1", Some(b"val2")).unwrap();
    txn.del(db.dbi(), b"key2", None).unwrap();
    txn.commit().unwrap();

    {
        let txn = env.begin_ro_txn().unwrap();
        let dbi = txn.open_db(None).unwrap().dbi();
        let stat = txn.db_stat(dbi).unwrap();
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
        let dbi = txn.open_db(None).unwrap().dbi();
        let stat = txn.db_stat(dbi).unwrap();
        assert_eq!(stat.entries(), 8);
    }
}

#[test]
fn test_get_many() {
    let dir = tempdir().unwrap();
    let env = Environment::builder().set_max_dbs(4).open(dir.path()).unwrap();
    let key = |i: u32| i.wrapping_mul(2_654_435_761).to_be_bytes();
    let val = |i: u32| vec![i as u8; if i.is_multiple_of(97) { 5000 } else { 120 }];
    let probes: Vec<_> = (0..300_000u32).map(|i| key(i.wrapping_mul(7))).collect();
    let probes: Vec<&[u8]> = probes.iter().map(|k| &k[..]).collect();

    let txn = env.begin_rw_txn().unwrap();
    let db = txn.create_db(Some("plain"), DatabaseFlags::empty()).unwrap().dbi();
    let dup = txn.create_db(Some("dup"), DatabaseFlags::DUP_SORT).unwrap().dbi();
    // An empty table finds nothing.
    txn.get_many::<Vec<u8>>(db, &probes, |_, v| assert!(v.is_none())).unwrap();
    for i in 0..150_000u32 {
        txn.put(db, key(i), val(i), WriteFlags::empty()).unwrap();
        txn.put(dup, key(i % 1000), key(i), WriteFlags::empty()).unwrap();
    }
    // The table is large enough for interleaved searches.
    let stat = txn.db_stat_with_dbi(db).unwrap();
    assert!((stat.branch_pages() + stat.leaf_pages()) * stat.page_size() as usize >= 16 << 20);
    // Dirty pages of a write transaction.
    check_get_many(&txn, db, &probes);
    // Batches too small to interleave.
    check_get_many(&txn, db, &probes[..3]);
    check_get_many(&txn, dup, &probes);
    txn.commit().unwrap();

    let txn = env.begin_ro_txn().unwrap();
    check_get_many(&txn, db, &probes);
    check_get_many(&txn, dup, &probes);
}

fn check_get_many<K: TransactionKind>(txn: &Transaction<K>, dbi: ffi::MDBX_dbi, probes: &[&[u8]]) {
    let mut found = Vec::new();
    txn.get_many::<Vec<u8>>(dbi, probes, |i, v| found.push((i, v))).unwrap();
    assert_eq!(found.len(), probes.len());
    for (j, (k, (i, v))) in probes.iter().zip(found).enumerate() {
        assert_eq!(i, j);
        assert_eq!(txn.get::<Vec<u8>>(dbi, k).unwrap(), v);
    }
}

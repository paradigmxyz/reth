#![allow(unreachable_pub)]
use reth_libmdbx::{Environment, WriteFlags};
use tempfile::{tempdir, TempDir};

pub fn get_key(n: u32) -> String {
    format!("key{n}")
}

pub fn get_data(n: u32) -> String {
    format!("data{n}")
}

pub fn setup_bench_db(num_rows: u32) -> (TempDir, Environment) {
    let dir = tempdir().unwrap();
    let env = Environment::builder().open(dir.path()).unwrap();

    {
        let txn = env.begin_rw_txn().unwrap();
        let db = txn.open_db(None).unwrap();
        for i in 0..num_rows {
            txn.put(db.dbi(), get_key(i), get_data(i), WriteFlags::empty()).unwrap();
        }
        txn.commit().unwrap();
    }
    (dir, env)
}

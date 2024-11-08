#![allow(missing_docs)]
#![cfg(feature = "test-utils")]

use alloy_primitives::Bytes;
use reth_db::{test_utils::create_test_rw_db_with_path, DatabaseEnv};
use reth_db_api::{
    table::{Compress, Encode, Table, TableRow},
    transaction::DbTxMut,
    Database,
};
use reth_fs_util as fs;
use std::{path::Path, sync::Arc};

/// Path where the DB is initialized for benchmarks.
#[allow(dead_code)]
pub(crate) const BENCH_DB_PATH: &str = "/tmp/reth-benches";

/// Used for `RandomRead` and `RandomWrite` benchmarks.
#[allow(dead_code)]
pub(crate) const RANDOM_INDEXES: [usize; 10] = [23, 2, 42, 5, 3, 99, 54, 0, 33, 64];

/// Returns bench vectors in the format: `Vec<(Key, EncodedKey, Value, CompressedValue)>`.
#[allow(dead_code)]
pub(crate) fn load_vectors<T: Table>() -> Vec<(T::Key, Bytes, T::Value, Bytes)>
where
    T::Key: Default + Clone + for<'de> serde::Deserialize<'de>,
    T::Value: Default + Clone + for<'de> serde::Deserialize<'de>,
{
    let path =
        format!("{}/../../../testdata/micro/db/{}.json", env!("CARGO_MANIFEST_DIR"), T::NAME);
    let list: Vec<TableRow<T>> = serde_json::from_reader(std::io::BufReader::new(
        std::fs::File::open(&path)
        .unwrap_or_else(|_| panic!("Test vectors not found. They can be generated from the workspace by calling `cargo run --bin reth --features dev -- test-vectors tables`: {:?}", path))
    ))
    .unwrap();

    list.into_iter()
        .map(|(k, v)| {
            (
                k.clone(),
                Bytes::copy_from_slice(k.encode().as_ref()),
                v.clone(),
                Bytes::copy_from_slice(v.compress().as_ref()),
            )
        })
        .collect::<Vec<_>>()
}

/// Sets up a clear database at `bench_db_path`.
#[allow(clippy::ptr_arg)]
#[allow(dead_code)]
pub(crate) fn set_up_db<T>(
    bench_db_path: &Path,
    pair: &Vec<(<T as Table>::Key, Bytes, <T as Table>::Value, Bytes)>,
) -> DatabaseEnv
where
    T: Table,
    T::Key: Default + Clone,
    T::Value: Default + Clone,
{
    // Reset DB
    let _ = fs::remove_dir_all(bench_db_path);
    let db = Arc::try_unwrap(create_test_rw_db_with_path(bench_db_path)).unwrap();

    {
        // Prepare data to be read
        let tx = db.tx_mut().expect("tx");
        for (k, _, v, _) in pair.clone() {
            tx.put::<T>(k, v).expect("submit");
        }
        tx.inner.commit().unwrap();
    }

    db.into_inner_db()
}

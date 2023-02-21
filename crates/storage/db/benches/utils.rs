use reth_db::{
    database::Database,
    mdbx::{test_utils::create_test_db_with_path, EnvKind, WriteMap},
    table::*,
    transaction::{DbTx, DbTxMut},
};
use std::path::Path;

/// Path where the DB is initialized for benchmarks.
const BENCH_DB_PATH: &str = "/tmp/reth-benches";

/// Used for RandomRead and RandomWrite benchmarks.
const RANDOM_INDEXES: [usize; 10] = [23, 2, 42, 5, 3, 99, 54, 0, 33, 64];

/// Returns bench vectors in the format: `Vec<(Key, EncodedKey, Value, CompressedValue)>`.
fn load_vectors<T: reth_db::table::Table>() -> Vec<(T::Key, bytes::Bytes, T::Value, bytes::Bytes)>
where
    T: Default,
    T::Key: Default + Clone + for<'de> serde::Deserialize<'de>,
    T::Value: Default + Clone + for<'de> serde::Deserialize<'de>,
{
    let list: Vec<(T::Key, T::Value)> = serde_json::from_reader(std::io::BufReader::new(
        std::fs::File::open(format!(
            "{}/../../../testdata/micro/db/{}.json",
            env!("CARGO_MANIFEST_DIR"),
            T::NAME
        ))
        .expect("Test vectors not found. They can be generated from the workspace by calling `cargo run --bin reth -- test-vectors tables`."),
    ))
    .unwrap();

    list.into_iter()
        .map(|(k, v)| {
            (
                k.clone(),
                bytes::Bytes::copy_from_slice(k.encode().as_ref()),
                v.clone(),
                bytes::Bytes::copy_from_slice(v.compress().as_ref()),
            )
        })
        .collect::<Vec<_>>()
}

/// Sets up a clear database at `bench_db_path`.
#[allow(clippy::ptr_arg)]
fn set_up_db<T>(
    bench_db_path: &Path,
    pair: &Vec<(<T as Table>::Key, bytes::Bytes, <T as Table>::Value, bytes::Bytes)>,
) -> reth_db::mdbx::Env<WriteMap>
where
    T: Table + Default,
    T::Key: Default + Clone,
    T::Value: Default + Clone,
{
    // Reset DB
    let _ = std::fs::remove_dir_all(bench_db_path);
    let db = create_test_db_with_path::<WriteMap>(EnvKind::RW, bench_db_path);

    {
        // Prepare data to be read
        let tx = db.tx_mut().expect("tx");
        for (k, _, v, _) in pair.clone() {
            tx.put::<T>(k, v).expect("submit");
        }
        tx.inner.commit().unwrap();
    }

    db
}

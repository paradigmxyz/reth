#![allow(missing_docs)]

use std::{collections::HashSet, path::Path, sync::Arc};

use criterion::{
    black_box, criterion_group, criterion_main, measurement::WallTime, BenchmarkGroup, Criterion,
};
use pprof::criterion::{Output, PProfProfiler};
use proptest::{
    arbitrary::Arbitrary,
    prelude::{any_with, ProptestConfig},
    strategy::{Strategy, ValueTree},
    test_runner::TestRunner,
};
use reth_db::{test_utils::create_test_rw_db_with_path, DatabaseEnv, TransactionHashNumbers};
use reth_db_api::{
    cursor::DbCursorRW,
    database::Database,
    table::{Table, TableRow},
    transaction::DbTxMut,
};
use reth_fs_util as fs;

mod utils;
use utils::*;

criterion_group! {
    name = benches;
    config = Criterion::default().with_profiler(PProfProfiler::new(100, Output::Flamegraph(None)));
    targets = hash_keys
}
criterion_main!(benches);

/// It benchmarks the insertion of rows into a table where `Keys` are hashes.
/// * `append`: Table is empty. Sorts during benchmark.
/// * `insert_sorted`: Table is preloaded with rows (same as batch size). Sorts during benchmark.
/// * `insert_unsorted`: Table is preloaded with rows (same as batch size).
/// * `put_sorted`: Table is preloaded with rows (same as batch size). Sorts during benchmark.
/// * `put_unsorted`: Table is preloaded with rows (same as batch size).
///
/// It does the above steps with different batches of rows. `10_000`, `100_000`, `1_000_000`. In the
/// end, the table statistics are shown (eg. number of pages, table size...)
pub fn hash_keys(c: &mut Criterion) {
    let mut group = c.benchmark_group("Hash-Keys Table Insertion");

    group.sample_size(10);

    for size in [10_000, 100_000, 1_000_000] {
        measure_table_insertion::<TransactionHashNumbers>(&mut group, size);
    }
}

fn measure_table_insertion<T>(group: &mut BenchmarkGroup<'_, WallTime>, size: usize)
where
    T: Table,
    T::Key: Default
        + Clone
        + for<'de> serde::Deserialize<'de>
        + Arbitrary
        + serde::Serialize
        + Ord
        + std::hash::Hash,
    T::Value: Default + Clone + for<'de> serde::Deserialize<'de> + Arbitrary + serde::Serialize,
{
    let bench_db_path = Path::new(BENCH_DB_PATH);

    let scenarios: Vec<(fn(_, _) -> _, &str)> = vec![
        (append::<T>, "append_all"),
        (append::<T>, "append_input"),
        (insert::<T>, "insert_unsorted"),
        (insert::<T>, "insert_sorted"),
        (put::<T>, "put_unsorted"),
        (put::<T>, "put_sorted"),
    ];

    // `preload` is to be inserted into the database during the setup phase in all scenarios but
    // `append`.
    let (preload, unsorted_input) = generate_batches::<T>(size);

    for (scenario, scenario_str) in scenarios {
        // Append does not preload the table
        let mut preload_size = size;
        let mut input_size = size;
        if scenario_str.contains("append") {
            if scenario_str == "append_all" {
                input_size = size * 2;
            }
            preload_size = 0;
        }

        // Setup phase before each benchmark iteration
        let setup = || {
            // Reset DB
            let _ = fs::remove_dir_all(bench_db_path);
            let db = Arc::try_unwrap(create_test_rw_db_with_path(bench_db_path)).unwrap();
            let db = db.into_inner_db();

            let mut unsorted_input = unsorted_input.clone();
            if scenario_str == "append_all" {
                unsorted_input.extend_from_slice(&preload);
            }

            if preload_size > 0 {
                db.update(|tx| {
                    for (key, value) in &preload {
                        let _ = tx.put::<T>(key.clone(), value.clone());
                    }
                })
                .unwrap();
            }

            (unsorted_input, db)
        };

        // Iteration to be benchmarked
        let execution = |(input, db)| {
            let mut input: Vec<TableRow<T>> = input;
            if scenario_str.contains("_sorted") || scenario_str.contains("append") {
                input.sort_by(|a, b| a.0.cmp(&b.0));
            }
            scenario(db, input)
        };

        group.bench_function(
            format!(
                "{} |  {scenario_str} | preload: {} | writing: {} ",
                T::NAME,
                preload_size,
                input_size
            ),
            |b| {
                b.iter_with_setup(setup, execution);
            },
        );

        // Execute once more to show table stats (doesn't count for benchmarking speed)
        let db = execution(setup());
        get_table_stats::<T>(db);
    }
}

/// Generates two batches. The first is to be inserted into the database before running the
/// benchmark. The second is to be benchmarked with.
fn generate_batches<T>(size: usize) -> (Vec<TableRow<T>>, Vec<TableRow<T>>)
where
    T: Table,
    T::Key: std::hash::Hash + Arbitrary,
    T::Value: Arbitrary,
{
    let strategy = proptest::collection::vec(
        any_with::<TableRow<T>>((
            <T::Key as Arbitrary>::Parameters::default(),
            <T::Value as Arbitrary>::Parameters::default(),
        )),
        size,
    )
    .no_shrink()
    .boxed();

    let mut runner = TestRunner::new(ProptestConfig::default());
    let mut preload = strategy.new_tree(&mut runner).unwrap().current();
    let mut input = strategy.new_tree(&mut runner).unwrap().current();

    let mut unique_keys = HashSet::with_capacity(preload.len() + input.len());
    preload.retain(|(k, _)| unique_keys.insert(k.clone()));
    input.retain(|(k, _)| unique_keys.insert(k.clone()));

    (preload, input)
}

fn append<T>(db: DatabaseEnv, input: Vec<(<T as Table>::Key, <T as Table>::Value)>) -> DatabaseEnv
where
    T: Table,
{
    {
        let tx = db.tx_mut().expect("tx");
        let mut crsr = tx.cursor_write::<T>().expect("cursor");
        black_box({
            for (k, v) in input {
                crsr.append(k, v).expect("submit");
            }

            tx.inner.commit().unwrap()
        });
    }
    db
}

fn insert<T>(db: DatabaseEnv, input: Vec<(<T as Table>::Key, <T as Table>::Value)>) -> DatabaseEnv
where
    T: Table,
{
    {
        let tx = db.tx_mut().expect("tx");
        let mut crsr = tx.cursor_write::<T>().expect("cursor");
        black_box({
            for (k, v) in input {
                crsr.insert(k, v).expect("submit");
            }

            tx.inner.commit().unwrap()
        });
    }
    db
}

fn put<T>(db: DatabaseEnv, input: Vec<(<T as Table>::Key, <T as Table>::Value)>) -> DatabaseEnv
where
    T: Table,
{
    {
        let tx = db.tx_mut().expect("tx");
        black_box({
            for (k, v) in input {
                tx.put::<T>(k, v).expect("submit");
            }

            tx.inner.commit().unwrap()
        });
    }
    db
}

#[derive(Debug)]
#[allow(dead_code)]
struct TableStats {
    page_size: usize,
    leaf_pages: usize,
    branch_pages: usize,
    overflow_pages: usize,
    num_pages: usize,
    size: usize,
}

fn get_table_stats<T>(db: DatabaseEnv)
where
    T: Table,
{
    db.view(|tx| {
        let table_db = tx.inner.open_db(Some(T::NAME)).map_err(|_| "Could not open db.").unwrap();

        println!(
            "{:?}\n",
            tx.inner
                .db_stat(&table_db)
                .map_err(|_| format!("Could not find table: {}", T::NAME))
                .map(|stats| {
                    let num_pages =
                        stats.leaf_pages() + stats.branch_pages() + stats.overflow_pages();
                    let size = num_pages * stats.page_size() as usize;

                    TableStats {
                        page_size: stats.page_size() as usize,
                        leaf_pages: stats.leaf_pages(),
                        branch_pages: stats.branch_pages(),
                        overflow_pages: stats.overflow_pages(),
                        num_pages,
                        size,
                    }
                })
                .unwrap()
        );
    })
    .unwrap();
}

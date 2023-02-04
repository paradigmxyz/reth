#![allow(dead_code, unused_imports, non_snake_case)]

use criterion::{
    black_box, criterion_group, criterion_main, measurement::WallTime, BenchmarkGroup, Criterion,
};
use proptest::{
    arbitrary::Arbitrary,
    prelude::{any_with, ProptestConfig},
    strategy::{Strategy, ValueTree},
    test_runner::TestRunner,
};
use reth_db::{
    cursor::{DbDupCursorRO, DbDupCursorRW},
    mdbx::Env,
};
use std::{collections::HashSet, time::Instant};

criterion_group!(benches, hash_keys);
criterion_main!(benches);

pub fn hash_keys(c: &mut Criterion) {
    let mut group = c.benchmark_group("Hash-Keys Table Insertion");
    group.measurement_time(std::time::Duration::from_millis(200));
    group.warm_up_time(std::time::Duration::from_millis(200));

    measure_table_insertion::<TxHashNumber>(&mut group);
}

fn measure_table_insertion<T>(group: &mut BenchmarkGroup<WallTime>)
where
    T: Table + Default,
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

    let scenarios: Vec<(fn(_, _) -> (), &str)> = vec![
        (append::<T>, "append"),
        (insert::<T>, "insert_unsorted"),
        (insert::<T>, "insert_sorted"),
        (put::<T>, "put_unsorted"),
        (put::<T>, "put_sorted"),
    ];

    // `preload` is to be inserted into the database during the setup phase in all scenarios but
    // `append`.
    let (preload, unsorted_input) = generate_batches::<T>();

    for (scenario, scenario_str) in scenarios {
        group.bench_function(format!("{} | {scenario_str}", T::NAME), |b| {
            b.iter_with_setup(
                || {
                    // Reset DB
                    let _ = std::fs::remove_dir_all(bench_db_path);
                    let db = create_test_db_with_path::<WriteMap>(EnvKind::RW, bench_db_path);

                    if scenario_str != "append" {
                        db.update(|tx| {
                            for (key, value) in &preload {
                                let _ = tx.put::<T>(key.clone(), value.clone());
                            }
                        })
                        .unwrap();
                    }

                    (unsorted_input.clone(), db)
                },
                |(mut input, db)| {
                    if scenario_str.contains("_sorted") || scenario_str.contains("append") {
                        input.sort_by(|a, b| a.0.cmp(&b.0));
                    }
                    scenario(db, input)
                },
            )
        });
    }
}

/// Generates two batches. The first is to be inserted into the database before running the
/// benchmark. The second is to be benchmarked with.
fn generate_batches<T>() -> (Vec<(T::Key, T::Value)>, Vec<(T::Key, T::Value)>)
where
    T: Table + Default,
    T::Key: std::hash::Hash + Arbitrary,
    T::Value: Arbitrary,
{
    let strat = proptest::collection::vec(
        any_with::<(T::Key, T::Value)>((
            <T::Key as Arbitrary>::Parameters::default(),
            <T::Value as Arbitrary>::Parameters::default(),
        )),
        50_000,
    )
    .no_shrink()
    .boxed();

    let mut runner = TestRunner::new(ProptestConfig::default());
    let mut preload = strat.new_tree(&mut runner).unwrap().current();
    let mut input = strat.new_tree(&mut runner).unwrap().current();

    let mut unique_keys = HashSet::new();
    preload.retain(|(k, _)| unique_keys.insert(k.clone()));
    input.retain(|(k, _)| unique_keys.insert(k.clone()));

    (preload, input)
}

fn append<T>(db: Env<WriteMap>, input: Vec<(<T as Table>::Key, <T as Table>::Value)>)
where
    T: Table + Default,
{
    let tx = db.tx_mut().expect("tx");
    let mut crsr = tx.cursor_write::<T>().expect("cursor");
    black_box({
        for (k, v) in input {
            crsr.append(k, v).expect("submit");
        }

        tx.inner.commit().unwrap();
    });
}

fn insert<T>(db: Env<WriteMap>, input: Vec<(<T as Table>::Key, <T as Table>::Value)>)
where
    T: Table + Default,
{
    let tx = db.tx_mut().expect("tx");
    let mut crsr = tx.cursor_write::<T>().expect("cursor");
    black_box({
        for (k, v) in input {
            crsr.insert(k, v).expect("submit");
        }

        tx.inner.commit().unwrap();
    });
}

fn put<T>(db: Env<WriteMap>, input: Vec<(<T as Table>::Key, <T as Table>::Value)>)
where
    T: Table + Default,
{
    let tx = db.tx_mut().expect("tx");
    black_box({
        for (k, v) in input {
            tx.put::<T>(k, v).expect("submit");
        }

        tx.inner.commit().unwrap();
    });
}

include!("./utils.rs");

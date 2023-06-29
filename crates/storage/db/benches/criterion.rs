#![allow(dead_code, unused_imports, non_snake_case)]

use criterion::{
    black_box, criterion_group, criterion_main, measurement::WallTime, BenchmarkGroup, Criterion,
};
use pprof::criterion::{Output, PProfProfiler};
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
    tables::*,
};
use std::time::Instant;

criterion_group! {
    name = benches;
    config = Criterion::default().with_profiler(PProfProfiler::new(100, Output::Flamegraph(None)));
    targets = db, serialization
}
criterion_main!(benches);

pub fn db(c: &mut Criterion) {
    let mut group = c.benchmark_group("tables_db");
    group.measurement_time(std::time::Duration::from_millis(200));
    group.warm_up_time(std::time::Duration::from_millis(200));

    measure_table_db::<CanonicalHeaders>(&mut group);
    measure_table_db::<HeaderTD>(&mut group);
    measure_table_db::<HeaderNumbers>(&mut group);
    measure_table_db::<Headers>(&mut group);
    measure_table_db::<BlockBodyIndices>(&mut group);
    measure_table_db::<BlockOmmers>(&mut group);
    measure_table_db::<TxHashNumber>(&mut group);
    measure_table_db::<Transactions>(&mut group);
    measure_dupsort_db::<PlainStorageState>(&mut group);
    measure_table_db::<PlainAccountState>(&mut group);
}

pub fn serialization(c: &mut Criterion) {
    let mut group = c.benchmark_group("tables_serialization");
    group.measurement_time(std::time::Duration::from_millis(200));
    group.warm_up_time(std::time::Duration::from_millis(200));

    measure_table_serialization::<CanonicalHeaders>(&mut group);
    measure_table_serialization::<HeaderTD>(&mut group);
    measure_table_serialization::<HeaderNumbers>(&mut group);
    measure_table_serialization::<Headers>(&mut group);
    measure_table_serialization::<BlockBodyIndices>(&mut group);
    measure_table_serialization::<BlockOmmers>(&mut group);
    measure_table_serialization::<TxHashNumber>(&mut group);
    measure_table_serialization::<Transactions>(&mut group);
    measure_table_serialization::<PlainStorageState>(&mut group);
    measure_table_serialization::<PlainAccountState>(&mut group);
}

/// Measures `Encode`, `Decode`, `Compress` and `Decompress`.
fn measure_table_serialization<T>(group: &mut BenchmarkGroup<WallTime>)
where
    T: Table + Default,
    T::Key: Default + Clone + for<'de> serde::Deserialize<'de>,
    T::Value: Default + Clone + for<'de> serde::Deserialize<'de>,
{
    let input = &load_vectors::<T>();
    group.bench_function(format!("{}.KeyEncode", T::NAME), move |b| {
        b.iter_with_setup(
            || input.clone(),
            |input| {
                {
                    for (k, _, _, _) in input {
                        k.encode();
                    }
                };
                black_box(());
            },
        )
    });

    group.bench_function(format!("{}.KeyDecode", T::NAME), |b| {
        b.iter_with_setup(
            || input.clone(),
            |input| {
                {
                    for (_, k, _, _) in input {
                        let _ = <T as Table>::Key::decode(k);
                    }
                };
                black_box(());
            },
        )
    });

    group.bench_function(format!("{}.ValueCompress", T::NAME), move |b| {
        b.iter_with_setup(
            || input.clone(),
            |input| {
                {
                    for (_, _, v, _) in input {
                        v.compress();
                    }
                };
                black_box(());
            },
        )
    });

    group.bench_function(format!("{}.ValueDecompress", T::NAME), |b| {
        b.iter_with_setup(
            || input.clone(),
            |input| {
                {
                    for (_, _, _, v) in input {
                        let _ = <T as Table>::Value::decompress(v);
                    }
                };
                black_box(());
            },
        )
    });
}

/// Measures `SeqWrite`, `RandomWrite`, `SeqRead` and `RandomRead` using `cursor` and `tx.put`.
fn measure_table_db<T>(group: &mut BenchmarkGroup<WallTime>)
where
    T: Table + Default,
    T::Key: Default + Clone + for<'de> serde::Deserialize<'de>,
    T::Value: Default + Clone + for<'de> serde::Deserialize<'de>,
{
    let input = &load_vectors::<T>();
    let bench_db_path = Path::new(BENCH_DB_PATH);

    group.bench_function(format!("{}.SeqWrite", T::NAME), |b| {
        b.iter_with_setup(
            || {
                // Reset DB
                let _ = std::fs::remove_dir_all(bench_db_path);
                (
                    input.clone(),
                    Arc::try_unwrap(create_test_rw_db_with_path(bench_db_path)).unwrap(),
                )
            },
            |(input, db)| {
                // Create TX
                let tx = db.tx_mut().expect("tx");
                let mut crsr = tx.cursor_write::<T>().expect("cursor");

                black_box({
                    for (k, _, v, _) in input {
                        crsr.append(k, v).expect("submit");
                    }

                    tx.inner.commit().unwrap()
                });
            },
        )
    });

    group.bench_function(format!("{}.RandomWrite", T::NAME), |b| {
        b.iter_with_setup(
            || {
                // Reset DB
                let _ = std::fs::remove_dir_all(bench_db_path);
                (input, Arc::try_unwrap(create_test_rw_db_with_path(bench_db_path)).unwrap())
            },
            |(input, db)| {
                // Create TX
                let tx = db.tx_mut().expect("tx");
                let mut crsr = tx.cursor_write::<T>().expect("cursor");

                black_box({
                    for index in RANDOM_INDEXES {
                        let (k, _, v, _) = input.get(index).unwrap().clone();
                        crsr.insert(k, v).expect("submit");
                    }

                    tx.inner.commit().unwrap()
                });
            },
        )
    });

    group.bench_function(format!("{}.SeqRead", T::NAME), |b| {
        let db = set_up_db::<T>(bench_db_path, input);

        b.iter(|| {
            // Create TX
            let tx = db.tx().expect("tx");

            {
                let mut cursor = tx.cursor_read::<T>().expect("cursor");
                let walker = cursor.walk(Some(input.first().unwrap().0.clone())).unwrap();
                for element in walker {
                    element.unwrap();
                }
            };
            black_box(());
        })
    });

    group.bench_function(format!("{}.RandomRead", T::NAME), |b| {
        let db = set_up_db::<T>(bench_db_path, input);

        b.iter(|| {
            // Create TX
            let tx = db.tx().expect("tx");

            {
                for index in RANDOM_INDEXES {
                    let mut cursor = tx.cursor_read::<T>().expect("cursor");
                    cursor.seek_exact(input.get(index).unwrap().0.clone()).unwrap();
                }
            };
            black_box(());
        })
    });
}

/// Measures `SeqWrite`, `RandomWrite` and `SeqRead`  using `cursor_dup` and `tx.put`.
fn measure_dupsort_db<T>(group: &mut BenchmarkGroup<WallTime>)
where
    T: Table + Default + DupSort,
    T::Key: Default + Clone + for<'de> serde::Deserialize<'de>,
    T::Value: Default + Clone + for<'de> serde::Deserialize<'de>,
    T::SubKey: Default + Clone + for<'de> serde::Deserialize<'de>,
{
    let input = &load_vectors::<T>();
    let bench_db_path = Path::new(BENCH_DB_PATH);

    group.bench_function(format!("{}.SeqWrite", T::NAME), |b| {
        b.iter_with_setup(
            || {
                // Reset DB
                let _ = std::fs::remove_dir_all(bench_db_path);
                (
                    input.clone(),
                    Arc::try_unwrap(create_test_rw_db_with_path(bench_db_path)).unwrap(),
                )
            },
            |(input, db)| {
                // Create TX
                let tx = db.tx_mut().expect("tx");
                let mut crsr = tx.cursor_dup_write::<T>().expect("cursor");

                black_box({
                    for (k, _, v, _) in input {
                        crsr.append_dup(k, v).expect("submit");
                    }

                    tx.inner.commit().unwrap()
                });
            },
        )
    });

    group.bench_function(format!("{}.RandomWrite", T::NAME), |b| {
        b.iter_with_setup(
            || {
                // Reset DB
                let _ = std::fs::remove_dir_all(bench_db_path);

                (input, Arc::try_unwrap(create_test_rw_db_with_path(bench_db_path)).unwrap())
            },
            |(input, db)| {
                // Create TX
                let tx = db.tx_mut().expect("tx");

                for index in RANDOM_INDEXES {
                    let (k, _, v, _) = input.get(index).unwrap().clone();
                    tx.put::<T>(k, v).unwrap();
                }

                tx.inner.commit().unwrap();
            },
        )
    });

    group.bench_function(format!("{}.SeqRead", T::NAME), |b| {
        let db = set_up_db::<T>(bench_db_path, input);

        b.iter(|| {
            // Create TX
            let tx = db.tx().expect("tx");

            {
                let mut cursor = tx.cursor_dup_read::<T>().expect("cursor");
                let walker = cursor.walk_dup(None, Some(T::SubKey::default())).unwrap();
                for element in walker {
                    element.unwrap();
                }
            };
            black_box(());
        })
    });

    // group.bench_function(format!("{}.RandomRead", T::NAME), |b| {});
}

include!("./utils.rs");

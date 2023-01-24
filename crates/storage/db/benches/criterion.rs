#![allow(dead_code, unused_imports, non_snake_case)]

use criterion::{
    black_box, criterion_group, criterion_main, measurement::WallTime, BenchmarkGroup, Criterion,
};
use std::time::Instant;

criterion_group!(benches, db, serialization);
criterion_main!(benches);

pub fn db(c: &mut Criterion) {
    let mut group = c.benchmark_group("tables_db");
    group.measurement_time(std::time::Duration::from_millis(200));
    group.warm_up_time(std::time::Duration::from_millis(200));

    measure_table_db::<CanonicalHeaders>(&mut group);
    measure_table_db::<HeaderTD>(&mut group);
    measure_table_db::<HeaderNumbers>(&mut group);
    measure_table_db::<Headers>(&mut group);
    measure_table_db::<BlockBodies>(&mut group);
    measure_table_db::<BlockOmmers>(&mut group);
    measure_table_db::<TxHashNumber>(&mut group);
    measure_table_db::<BlockTransitionIndex>(&mut group);
    measure_table_db::<TxTransitionIndex>(&mut group);
    measure_table_db::<Transactions>(&mut group);
    measure_table_db::<PlainStorageState>(&mut group);
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
    measure_table_serialization::<BlockBodies>(&mut group);
    measure_table_serialization::<BlockOmmers>(&mut group);
    measure_table_serialization::<TxHashNumber>(&mut group);
    measure_table_serialization::<BlockTransitionIndex>(&mut group);
    measure_table_serialization::<TxTransitionIndex>(&mut group);
    measure_table_serialization::<Transactions>(&mut group);
    measure_table_serialization::<PlainStorageState>(&mut group);
    measure_table_serialization::<PlainAccountState>(&mut group);
}

fn measure_table_serialization<T>(group: &mut BenchmarkGroup<WallTime>)
where
    T: Table + Default,
    T::Key: Default + Clone + for<'de> serde::Deserialize<'de>,
    T::Value: Default + Clone + for<'de> serde::Deserialize<'de>,
{
    let pair = &load_vectors::<T>();
    group.bench_function(format!("{}.KeyEncode", T::NAME), move |b| {
        b.iter_with_setup(
            || pair.clone(),
            |pair| {
                black_box({
                    for (k, _, _, _) in pair {
                        k.encode();
                    }
                });
            },
        )
    });

    group.bench_function(format!("{}.KeyDecode", T::NAME), |b| {
        b.iter_with_setup(
            || pair.clone(),
            |pair| {
                black_box({
                    for (_, k, _, _) in pair {
                        let _ = <T as Table>::Key::decode(k);
                    }
                });
            },
        )
    });

    group.bench_function(format!("{}.ValueCompress", T::NAME), move |b| {
        b.iter_with_setup(
            || pair.clone(),
            |pair| {
                black_box({
                    for (_, _, v, _) in pair {
                        v.compress();
                    }
                });
            },
        )
    });

    group.bench_function(format!("{}.ValueDecompress", T::NAME), |b| {
        b.iter_with_setup(
            || pair.clone(),
            |pair| {
                black_box({
                    for (_, _, _, v) in pair {
                        let _ = <T as Table>::Value::decompress(v);
                    }
                });
            },
        )
    });
}

fn measure_table_db<T>(group: &mut BenchmarkGroup<WallTime>)
where
    T: Table + Default,
    T::Key: Default + Clone + for<'de> serde::Deserialize<'de>,
    T::Value: Default + Clone + for<'de> serde::Deserialize<'de>,
{
    let pair = &load_vectors::<T>();
    let bench_db_path = Path::new(BENCH_DB_PATH);

    group.bench_function(format!("{}.SeqWrite", T::NAME), |b| {
        b.iter_with_setup(
            || {
                // Reset DB
                let _ = std::fs::remove_dir_all(bench_db_path);
                let db = create_test_db_with_path::<WriteMap>(EnvKind::RW, bench_db_path);

                (pair.clone(), db)
            },
            |(pair, db)| {
                // Create TX
                let tx = db.tx_mut().expect("tx");
                let mut crsr = tx.cursor_write::<T>().expect("cursor");

                black_box({
                    for (k, _, v, _) in pair {
                        crsr.append(k, v).expect("submit");
                    }

                    tx.inner.commit().unwrap();
                });
            },
        )
    });

    group.bench_function(format!("{}.RandomWrite", T::NAME), |b| {
        b.iter_with_setup(
            || {
                // Reset DB
                let _ = std::fs::remove_dir_all(bench_db_path);
                let db = create_test_db_with_path::<WriteMap>(EnvKind::RW, bench_db_path);

                (
                    pair.iter()
                        .enumerate()
                        .filter(|(i, _)| RANDOM_INDEXES.contains(i))
                        .map(|(i, e)| (i, e.clone()))
                        .collect::<Vec<_>>(),
                    db,
                )
            },
            |(pair, db)| {
                // Create TX
                let tx = db.tx_mut().expect("tx");
                let mut crsr = tx.cursor_write::<T>().expect("cursor");

                black_box({
                    for (_, (k, _, v, _)) in pair {
                        crsr.insert(k, v).expect("submit");
                    }

                    tx.inner.commit().unwrap();
                });
            },
        )
    });

    group.bench_function(format!("{}.SeqRead", T::NAME), |b| {
        let db = set_up_db::<T>(bench_db_path, pair);

        b.iter(|| {
            // Create TX
            let tx = db.tx().expect("tx");

            black_box({
                let mut cursor = tx.cursor_read::<T>().expect("cursor");
                let walker = cursor.walk(pair.first().unwrap().0.clone()).unwrap();
                for element in walker {
                    element.unwrap();
                }
            });
        })
    });

    group.bench_function(format!("{}.RandomRead", T::NAME), |b| {
        let db = set_up_db::<T>(bench_db_path, pair);

        b.iter(|| {
            // Create TX
            let tx = db.tx().expect("tx");

            black_box({
                for index in RANDOM_INDEXES {
                    let mut cursor = tx.cursor_read::<T>().expect("cursor");
                    cursor.seek_exact(pair.get(index).unwrap().0.clone()).unwrap();
                }
            });
        })
    });
}

include!("./utils.rs");

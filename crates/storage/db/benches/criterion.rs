use criterion::{
    black_box, criterion_group, criterion_main, measurement::WallTime, BenchmarkGroup, Criterion,
};
use reth_db::{
    cursor::DbCursorRW,
    database::Database,
    mdbx::{test_utils::create_test_rw_db, WriteMap},
    table::*,
    tables::*,
    transaction::DbTxMut,
};
use std::time::Instant;

criterion_group!(benches, db, serialization);
criterion_main!(benches);

pub fn db(c: &mut Criterion) {
    let mut group = c.benchmark_group("tables_db");
    group.measurement_time(std::time::Duration::from_millis(100));
    group.warm_up_time(std::time::Duration::from_millis(100));

    measure_table_db::<CanonicalHeaders>(&mut group);
    measure_table_db::<HeaderTD>(&mut group);
    measure_table_db::<HeaderNumbers>(&mut group);
    measure_table_db::<Headers>(&mut group);
    measure_table_db::<BlockBodies>(&mut group);
    measure_table_db::<BlockOmmers>(&mut group);
    measure_table_db::<TxHashNumber>(&mut group);
    measure_table_db::<BlockTransitionIndex>(&mut group);
    measure_table_db::<TxTransitionIndex>(&mut group);
    measure_table_db::<SyncStage>(&mut group);
    measure_table_db::<Transactions>(&mut group);
    measure_table_db::<PlainStorageState>(&mut group);
    measure_table_db::<PlainAccountState>(&mut group);
}

pub fn serialization(c: &mut Criterion) {
    let mut group = c.benchmark_group("tables_serialization");
    group.measurement_time(std::time::Duration::from_millis(100));
    group.warm_up_time(std::time::Duration::from_millis(100));

    measure_table_serialization::<CanonicalHeaders>(&mut group);
    measure_table_serialization::<HeaderTD>(&mut group);
    measure_table_serialization::<HeaderNumbers>(&mut group);
    measure_table_serialization::<Headers>(&mut group);
    measure_table_serialization::<BlockBodies>(&mut group);
    measure_table_serialization::<BlockOmmers>(&mut group);
    measure_table_serialization::<TxHashNumber>(&mut group);
    measure_table_serialization::<BlockTransitionIndex>(&mut group);
    measure_table_serialization::<TxTransitionIndex>(&mut group);
    measure_table_serialization::<SyncStage>(&mut group);
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
        b.iter_custom(|_| {
            let pair = pair.clone();
            let timer = Instant::now();
            black_box({
                for (k, _, _, _) in pair {
                    k.encode();
                }
            });
            timer.elapsed()
        })
    });

    group.bench_function(format!("{}.KeyDecode", T::NAME), |b| {
        b.iter_custom(|_| {
            let pair = pair.clone();
            let timer = Instant::now();
            black_box({
                for (_, k, _, _) in pair {
                    let _ = <T as Table>::Key::decode(k);
                }
            });
            timer.elapsed()
        })
    });

    group.bench_function(format!("{}.ValueCompress", T::NAME), move |b| {
        b.iter_custom(|_| {
            let pair = pair.clone();
            let timer = Instant::now();
            black_box({
                for (_, _, v, _) in pair {
                    v.compress();
                }
            });
            timer.elapsed()
        })
    });

    group.bench_function(format!("{}.ValueDecompress", T::NAME), |b| {
        b.iter_custom(|_| {
            let pair = pair.clone();
            let timer = Instant::now();
            black_box({
                for (_, _, _, v) in pair {
                    let _ = <T as Table>::Value::decompress(v);
                }
            });
            timer.elapsed()
        })
    });
}

fn measure_table_db<T>(group: &mut BenchmarkGroup<WallTime>)
where
    T: Table + Default,
    T::Key: Default + Clone + for<'de> serde::Deserialize<'de>,
    T::Value: Default + Clone + for<'de> serde::Deserialize<'de>,
{
    let pair = &load_vectors::<T>();

    group.bench_function(format!("{}.SeqWrite", T::NAME), |b| {
        b.iter_custom(|_| {
            let pair = pair.clone();
            let timer = Instant::now();
            black_box({
                // TODO generic test db
                let db = create_test_rw_db::<WriteMap>();

                let tx = db.tx_mut().expect("tx");
                let mut crsr = tx.cursor_write::<T>().expect("cursor");

                // placeholder: cant insert multiple default values, that's why we're limiting to
                // one for now.
                for (k, _, v, _) in pair {
                    crsr.insert(k, v).expect("submit");
                }

                tx.inner.commit().unwrap();
            });
            timer.elapsed()
        })
    });
}

include!("./utils.rs");

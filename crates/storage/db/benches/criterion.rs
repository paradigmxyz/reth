use criterion::{
    criterion_group, criterion_main, measurement::WallTime, BenchmarkGroup, Criterion,
};
use reth_db::{
    cursor::DbCursorRW,
    database::Database,
    mdbx::{test_utils::create_test_rw_db, WriteMap},
    table::*,
    tables::*,
    transaction::DbTxMut,
};

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
    measure_table_db::<BlockOmmers>(&mut group);
    measure_table_db::<CumulativeTxCount>(&mut group);
    measure_table_db::<NonCanonicalTransactions>(&mut group);
    measure_table_db::<Transactions>(&mut group);
    measure_table_db::<TxHashNumber>(&mut group);
    measure_table_db::<Receipts>(&mut group);
    measure_table_db::<Logs>(&mut group);
    measure_table_db::<PlainAccountState>(&mut group);
    measure_table_db::<PlainStorageState>(&mut group);
    measure_table_db::<Bytecodes>(&mut group);
    measure_table_db::<AccountHistory>(&mut group);
    measure_table_db::<StorageHistory>(&mut group);
    measure_table_db::<AccountChangeSet>(&mut group);
    measure_table_db::<StorageChangeSet>(&mut group);
    measure_table_db::<TxSenders>(&mut group);
    measure_table_db::<Config>(&mut group);
    measure_table_db::<SyncStage>(&mut group);
}

pub fn serialization(c: &mut Criterion) {
    let mut group = c.benchmark_group("tables_serialization");
    group.measurement_time(std::time::Duration::from_millis(100));
    group.warm_up_time(std::time::Duration::from_millis(100));

    measure_table_serialization::<CanonicalHeaders>(&mut group);
    measure_table_serialization::<HeaderTD>(&mut group);
    measure_table_serialization::<HeaderNumbers>(&mut group);
    measure_table_serialization::<Headers>(&mut group);
    measure_table_serialization::<BlockOmmers>(&mut group);
    measure_table_serialization::<CumulativeTxCount>(&mut group);
    measure_table_serialization::<NonCanonicalTransactions>(&mut group);
    measure_table_serialization::<Transactions>(&mut group);
    measure_table_serialization::<TxHashNumber>(&mut group);
    measure_table_serialization::<Receipts>(&mut group);
    measure_table_serialization::<Logs>(&mut group);
    measure_table_serialization::<PlainAccountState>(&mut group);
    measure_table_serialization::<PlainStorageState>(&mut group);
    measure_table_serialization::<Bytecodes>(&mut group);
    measure_table_serialization::<AccountHistory>(&mut group);
    measure_table_serialization::<StorageHistory>(&mut group);
    measure_table_serialization::<AccountChangeSet>(&mut group);
    measure_table_serialization::<StorageChangeSet>(&mut group);
    measure_table_serialization::<TxSenders>(&mut group);
    measure_table_serialization::<Config>(&mut group);
    measure_table_serialization::<SyncStage>(&mut group);
}

fn measure_table_serialization<T>(group: &mut BenchmarkGroup<WallTime>)
where
    T: Table + Default,
    T::Key: Default + Clone,
    T::Value: Default + Clone,
{
    let pair = &load_vectors::<T>();
    group.bench_function(format!("{}.KeyEncode", T::NAME), |b| {
        b.iter(|| {
            for (k, _, _, _) in pair {
                k.clone().encode();
            }
        })
    });

    group.bench_function(format!("{}.KeyDecode", T::NAME), |b| {
        b.iter(|| {
            for (_, k, _, _) in pair {
                let _ = <T as Table>::Key::decode(k.clone());
            }
        })
    });

    group.bench_function(format!("{}.ValueCompress", T::NAME), move |b| {
        b.iter(move || {
            for (_, _, v, _) in pair {
                v.clone().compress();
            }
        })
    });

    group.bench_function(format!("{}.ValueDecompress", T::NAME), |b| {
        b.iter(|| {
            for (_, _, _, v) in pair {
                let _ = <T as Table>::Value::decompress(v.clone());
            }
        })
    });
}

fn measure_table_db<T>(group: &mut BenchmarkGroup<WallTime>)
where
    T: Table + Default,
    T::Key: Default + Clone,
    T::Value: Default + Clone,
{
    let pair = &load_vectors::<T>();

    group.bench_function(format!("{}.SeqWrite", T::NAME), |b| {
        b.iter(|| {
            // TODO generic test db
            let db = create_test_rw_db::<WriteMap>();

            let tx = db.tx_mut().expect("tx");
            let mut crsr = tx.cursor_mut::<T>().expect("cursor");

            // TODO sort kv before
            // placeholder: cant insert multiple default values, that's why we're limiting to one
            // for now.
            for (k, _, v, _) in &pair[..1] {
                crsr.insert(k.clone(), v.clone()).expect("submit");
            }

            tx.inner.commit().unwrap();
        })
    });
}

/// Returns bench vectors in the format: `Vec<(Key, EncodedKey, Value, CompressedValue)>`.
///
/// TBD, so for now only loads 3 default values.
fn load_vectors<T>() -> Vec<(T::Key, bytes::Bytes, T::Value, bytes::Bytes)>
where
    T: Table + Default,
    T::Key: Default + Clone,
    T::Value: Default + Clone,
{
    let encoded: bytes::Bytes = bytes::Bytes::copy_from_slice(T::Key::default().encode().as_ref());
    let compressed: bytes::Bytes =
        bytes::Bytes::copy_from_slice(T::Value::default().compress().as_ref());

    let row = (T::Key::default(), encoded, T::Value::default(), compressed);
    vec![row.clone(), row.clone(), row]
}

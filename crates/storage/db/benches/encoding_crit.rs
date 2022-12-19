use criterion::{criterion_group, criterion_main, Criterion};
use reth_db::{
    cursor::DbCursorRW,
    database::Database,
    mdbx::{test_utils::create_test_rw_db, WriteMap},
    table::*,
    tables::*,
    transaction::DbTxMut,
};

/// Returns bench vectors in the format: `Vec<(Key, EncodedKey, Value, CompressedValue)>`.
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

macro_rules! impl_criterion {
    ($($name:tt),+) => {

        pub fn db(c: &mut Criterion) {
            let mut db_group = c.benchmark_group("tables_db");
            db_group.measurement_time(std::time::Duration::from_millis(100));
            db_group.warm_up_time(std::time::Duration::from_millis(100));

            $(
                let pair = &load_vectors::<$name>();
                db_group.bench_function(stringify!($name.SeqWrite), |b| {
                    b.iter(|| {
                        // TODO generic test db
                        let db = create_test_rw_db::<WriteMap>();

                        let tx = db.tx_mut().expect("tx");
                        let mut crsr = tx.cursor_mut::<reth_db::tables::$name>().expect("cursor");

                        // TODO sort kv before
                        // placeholder: cant insert multiple default values, that's why we're limiting to one for now.
                        for (k, _, v, _) in &pair[..1] {
                            crsr.insert(k.clone(), v.clone()).expect("submit");
                        }

                        tx.inner.commit().unwrap();

                    })
                });

            )+
            db_group.finish();

        }

        pub fn tables(c: &mut Criterion) {
            let mut ser_group = c.benchmark_group("tables_key_value");
            ser_group.warm_up_time(std::time::Duration::from_secs(1));
            ser_group.measurement_time(std::time::Duration::from_secs(1));

            $(

                let pair = &load_vectors::<$name>();
                ser_group.bench_function(stringify!($name.KeyEncode), |b| {
                    b.iter(|| {
                        for (k, _, _, _) in pair {
                            k.clone().encode();
                        }
                    })
                });

                ser_group.bench_function(stringify!($name.KeyDecode), |b| {
                    b.iter(|| {
                        for (_, k, _, _) in pair {
                            let _ = <$name as Table>::Key::decode(k.clone());
                        }
                    })
                });

                ser_group.bench_function(stringify!($name.ValueCompress), move |b| {
                    b.iter(move || {
                        for (_, _, v, _) in pair {
                            v.clone().compress();
                        }
                    })
                });

                ser_group.bench_function(stringify!($name.ValueDecompress), |b| {
                    b.iter(|| {
                        for (_, _, _, v) in pair {
                            let _ = <$name as Table>::Value::decompress(v.clone());
                        }
                    })
                });
            )+
            ser_group.finish();

        }

    };
}

impl_criterion!(AccountHistory);

criterion_group!(benches, db, serialization);
criterion_main!(benches);

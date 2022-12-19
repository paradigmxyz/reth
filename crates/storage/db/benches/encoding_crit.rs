use criterion::{criterion_group, criterion_main, Criterion};
use reth_db::{table::*, tables::*};

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
        pub fn bench_ser(c: &mut Criterion) {
            let mut group = c.benchmark_group("table_key_value");
            group.warm_up_time(std::time::Duration::from_secs(1));
            group.measurement_time(std::time::Duration::from_secs(1));

            $(

                let pair = &load_vectors::<$name>();
                group.bench_function(stringify!($name.KeyEncode), |b| {
                    b.iter(|| {
                        for (k, _, _, _) in pair {
                            k.clone().encode();
                        }
                    })
                });

                group.bench_function(stringify!($name.KeyDecode), |b| {
                    b.iter(|| {
                        for (_, k, _, _) in pair {
                            let _ = <$name as Table>::Key::decode(k.clone());
                        }
                    })
                });

                group.bench_function(stringify!($name.ValueCompress), move |b| {
                    b.iter(move || {
                        for (_, _, v, _) in pair {
                            v.clone().compress();
                        }
                    })
                });

                group.bench_function(stringify!($name.ValueDecompress), |b| {
                    b.iter(|| {
                        for (_, _, _, v) in pair {
                            let _ = <$name as Table>::Value::decompress(v.clone());
                        }
                    })
                });

                // // TODO placeholder sort: fix
                // let db = mdbx::test_utils::create_test_rw_db();
                // let seq = pair.clone().sort();
                // let tx = db.tx_mut();
                // c.bench_function(stringify!($name.SeqWrite), |b| {
                //     b.iter(|| {
                //         for (k, _, v, _) in pair {
                //             <$name as Table>::Value::decompress(v.clone());
                //         }
                //     })
                // });
            )+
            group.finish();

        }

    };
}

impl_criterion!(AccountHistory);

criterion_group!(benches, bench_ser);
criterion_main!(benches);

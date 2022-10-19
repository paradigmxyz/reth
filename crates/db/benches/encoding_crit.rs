use criterion::{black_box, criterion_group, criterion_main, Criterion};

macro_rules! impl_benchmark {
    ($name:tt) => {
        pub fn criterion_benchmark(c: &mut Criterion) {
            let mut size = 0;
            c.bench_function(stringify!($name), |b| {
                b.iter(|| {
                    let (encoded_size, header) =
                        reth_db::kv::codecs::fuzz::Header::encode_and_decode(black_box(
                            reth_primitives::Header::default(),
                        ));

                    if size == 0 {
                        println!("{header:?}");
                        println!("{encoded_size}");

                        size = encoded_size;
                    }
                })
            });
            println!("Size (bytes): `{size}`");
        }

        criterion_group! {
            name = benches;
            config = Criterion::default().sample_size(10).measurement_time(std::time::Duration::from_millis(100));
            targets = criterion_benchmark
        }

        criterion_main!(benches);
    };
}

#[cfg(not(feature = "bench-postcard"))]
impl_benchmark!(scale);

#[cfg(feature = "bench-postcard")]
impl_benchmark!(postcard);

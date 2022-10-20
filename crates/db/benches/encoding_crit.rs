use criterion::{black_box, criterion_group, criterion_main, Criterion};

/// Benchmarks the encoding and decoding of `Header` using criterion.
macro_rules! impl_criterion_encoding_benchmark {
    ($name:tt) => {
        pub fn criterion_benchmark(c: &mut Criterion) {
            let mut size = 0;
            c.bench_function(stringify!($name), |b| {
                b.iter(|| {
                    let encoded_size =
                        reth_interfaces::db::codecs::fuzz::Header::encode_and_decode(black_box(
                            reth_primitives::Header::default(),
                        ))
                        .0;

                    if size == 0 {
                        size = encoded_size;
                    }
                })
            });
            println!("Size (bytes): `{size}`");
        }

        criterion_group!(benches, criterion_benchmark);
        criterion_main!(benches);
    };
}

#[cfg(not(feature = "bench-postcard"))]
impl_criterion_encoding_benchmark!(scale);

#[cfg(feature = "bench-postcard")]
impl_criterion_encoding_benchmark!(postcard);

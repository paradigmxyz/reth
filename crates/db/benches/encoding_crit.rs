use criterion::{black_box, criterion_group, criterion_main, Criterion};

macro_rules! impl_benchmark {
    ($name:tt) => {
        pub fn criterion_benchmark(c: &mut Criterion) {
            c.bench_function(stringify!($name), |b| {
                b.iter(|| {
                    reth_db::kv::codecs::fuzz::Header::encode_and_decode(black_box(
                        reth_primitives::Header::default(),
                    ))
                })
            });
        }

        criterion_group!(benches, criterion_benchmark);
        criterion_main!(benches);
    };
}

#[cfg(not(feature = "bench-postcard"))]
impl_benchmark!(scale);

#[cfg(feature = "bench-postcard")]
impl_benchmark!(postcard);

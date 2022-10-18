use criterion::{black_box, criterion_group, criterion_main, Criterion};

macro_rules! impl_benchmark {
    ($name:tt) => {
        pub fn criterion_benchmark(c: &mut Criterion) {
            let mut total_size = 0;
            c.bench_function(stringify!($name), |b| {
                b.iter(|| {
                    total_size += reth_db::kv::codecs::fuzz::Header::encode_and_decode(black_box(
                        reth_primitives::Header::default(),
                    ))
                    .0;
                })
            });
            println!("Size (bytes): `{total_size}`");
        }

        criterion_group!(benches, criterion_benchmark);
        criterion_main!(benches);
    };
}

#[cfg(not(feature = "bench-postcard"))]
impl_benchmark!(scale);

#[cfg(feature = "bench-postcard")]
impl_benchmark!(postcard);

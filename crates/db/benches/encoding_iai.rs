use iai::{black_box, main};

macro_rules! impl_benchmark {
    ($name:tt) => {
        fn $name() {
            reth_db::kv::codecs::fuzz::Header::encode_and_decode(black_box(
                reth_primitives::Header::default(),
            ));
        }

        main!($name);
    };
}

#[cfg(not(feature = "bench-postcard"))]
impl_benchmark!(scale);

#[cfg(feature = "bench-postcard")]
impl_benchmark!(postcard);

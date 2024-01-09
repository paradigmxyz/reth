#![allow(missing_docs)]
use criterion::{criterion_group, criterion_main, Criterion};
use proptest::{prelude::*, strategy::ValueTree};
use reth_primitives::trie::Nibbles;
use std::{hint::black_box, time::Duration};

/// Benchmarks the nibble unpacking.
pub fn nibbles_benchmark(c: &mut Criterion) {
    let mut g = c.benchmark_group("nibbles");
    g.warm_up_time(Duration::from_secs(1));
    g.noise_threshold(0.02);

    g.bench_function("unpack/32", |b| {
        let bytes = get_bytes(32);
        b.iter(|| Nibbles::unpack(black_box(&bytes[..])))
    });
    g.bench_function("unpack/256", |b| {
        let bytes = get_bytes(256);
        b.iter(|| Nibbles::unpack(black_box(&bytes[..])))
    });
    g.bench_function("unpack/2048", |b| {
        let bytes = get_bytes(2048);
        b.iter(|| Nibbles::unpack(black_box(&bytes[..])))
    });

    g.bench_function("pack/32", |b| {
        let nibbles = get_nibbles(32);
        b.iter(|| black_box(&nibbles).pack())
    });
    g.bench_function("pack/256", |b| {
        let nibbles = get_nibbles(256);
        b.iter(|| black_box(&nibbles).pack())
    });
    g.bench_function("pack/2048", |b| {
        let nibbles = get_nibbles(2048);
        b.iter(|| black_box(&nibbles).pack())
    });

    g.bench_function("encode_path_leaf/31", |b| {
        let nibbles = get_nibbles(31);
        b.iter(|| black_box(&nibbles).encode_path_leaf(false))
    });
    g.bench_function("encode_path_leaf/256", |b| {
        let nibbles = get_nibbles(256);
        b.iter(|| black_box(&nibbles).encode_path_leaf(false))
    });
    g.bench_function("encode_path_leaf/2048", |b| {
        let nibbles = get_nibbles(2048);
        b.iter(|| black_box(&nibbles).encode_path_leaf(false))
    });
}

fn get_nibbles(len: usize) -> Nibbles {
    Nibbles::from_nibbles_unchecked(get_bytes(len))
}

fn get_bytes(len: usize) -> Vec<u8> {
    proptest::collection::vec(proptest::arbitrary::any::<u8>(), len)
        .new_tree(&mut Default::default())
        .unwrap()
        .current()
}

criterion_group!(benches, nibbles_benchmark);
criterion_main!(benches);

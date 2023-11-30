use criterion::{criterion_group, criterion_main, Criterion};
use reth_primitives::trie::Nibbles;
use std::hint::black_box;

/// Benchmarks the nibble unpacking.
pub fn nibbles_benchmark(c: &mut Criterion) {
    let mut g = c.benchmark_group("nibbles");

    g.bench_function("unpack/32", |b| {
        let raw = (0..32).collect::<Vec<u8>>();
        b.iter(|| Nibbles::unpack(black_box(&raw[..])))
    });
    g.bench_function("unpack/256", |b| {
        let raw = (0..=255).collect::<Vec<u8>>();
        b.iter(|| Nibbles::unpack(black_box(&raw[..])))
    });
    g.bench_function("unpack/4096", |b| {
        let raw = (0..=255).collect::<Vec<u8>>().repeat(4096 / 256);
        b.iter(|| Nibbles::unpack(black_box(&raw[..])))
    });

    g.bench_function("pack/32", |b| {
        let raw = Nibbles::unpack((0..32).collect::<Vec<u8>>());
        b.iter(|| black_box(&raw).pack())
    });
    g.bench_function("pack/256", |b| {
        let raw = Nibbles::unpack((0..=255).collect::<Vec<u8>>());
        b.iter(|| black_box(&raw).pack())
    });
    g.bench_function("pack/4096", |b| {
        let raw = Nibbles::unpack((0..=255).collect::<Vec<u8>>().repeat(4096 / 256));
        b.iter(|| black_box(&raw).pack())
    });
}

criterion_group!(benches, nibbles_benchmark);
criterion_main!(benches);

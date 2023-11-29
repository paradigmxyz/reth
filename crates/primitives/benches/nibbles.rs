use criterion::{criterion_group, criterion_main, Criterion};
use reth_primitives::trie::Nibbles;

/// Benchmarks the nibble unpacking.
pub fn nibbles_benchmark(c: &mut Criterion) {
    c.bench_function("Nibbles unpack", |b| {
        let raw = (1..=32).collect::<Vec<u8>>();
        b.iter(|| {
            Nibbles::unpack(&raw);
        })
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default();
    targets = nibbles_benchmark
}
criterion_main!(benches);

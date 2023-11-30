use criterion::{criterion_group, criterion_main, Criterion};
use reth_primitives::trie::Nibbles;

/// Benchmarks the nibble unpacking.
pub fn nibbles_benchmark(c: &mut Criterion) {
    let mut g = c.benchmark_group("nibbles");
    g.bench_function("unpack", |b| {
        let raw = (1..=32).collect::<Vec<u8>>();
        b.iter(|| Nibbles::unpack(&raw))
    });
}

criterion_group!(benches, nibbles_benchmark);
criterion_main!(benches);

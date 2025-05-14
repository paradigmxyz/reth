#![allow(missing_docs)]

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use nybbles::Nibbles;
use rand::Rng;
use reth_trie_sparse::PackedNibbles;

fn generate_nibbles(rng: &mut impl Rng, length: usize) -> (Nibbles, PackedNibbles) {
    // Generate random nibbles
    let nibbles: Vec<u8> = (0..length).map(|_| rng.random_range(0..16)).collect();

    // Create instances of both types with same values
    let nybbles_nibbles = Nibbles::from_nibbles_unchecked(nibbles.clone());
    let packed_nibbles = PackedNibbles::from_nibbles(nibbles);

    (nybbles_nibbles, packed_nibbles)
}

fn bench_eq_same_len(c: &mut Criterion) {
    let mut rng = rand::rng();
    let mut group = c.benchmark_group("eq_comparison_same_len");

    for size in [4, 8, 16, 32, 64] {
        // Generate test data for each size
        let (nybbles1, packed1) = generate_nibbles(&mut rng, size);
        let (nybbles2, packed2) = generate_nibbles(&mut rng, size);

        // Clone for identical comparison
        let nybbles1_clone = nybbles1.clone();
        let packed1_clone = packed1.clone();

        // Equal comparison for Nibbles
        group.bench_with_input(BenchmarkId::new("Nibbles_eq_true", size), &size, |b, _| {
            b.iter(|| nybbles1.eq(&nybbles1_clone))
        });

        // Equal comparison for PackedNibbles
        group.bench_with_input(BenchmarkId::new("PackedNibbles_eq_true", size), &size, |b, _| {
            b.iter(|| packed1.eq(&packed1_clone))
        });

        // Not equal comparison for Nibbles
        group.bench_with_input(BenchmarkId::new("Nibbles_eq_false", size), &size, |b, _| {
            b.iter(|| nybbles1.eq(&nybbles2))
        });

        // Not equal comparison for PackedNibbles
        group.bench_with_input(BenchmarkId::new("PackedNibbles_eq_false", size), &size, |b, _| {
            b.iter(|| packed1.eq(&packed2))
        });
    }

    group.finish();
}

fn bench_eq_diff_len(c: &mut Criterion) {
    let mut rng = rand::rng();
    let mut group = c.benchmark_group("eq_comparison_diff_len");

    for (len1, len2) in [(4, 8), (8, 16), (16, 32), (32, 64)] {
        // Generate test data for each size
        let (nybbles1, packed1) = generate_nibbles(&mut rng, len1);
        let (nybbles2, packed2) = generate_nibbles(&mut rng, len2);

        let benchmark_name = format!("{}_{}", len1, len2);

        // Not equal comparison for Nibbles (different lengths)
        group.bench_with_input(
            BenchmarkId::new("Nibbles_diff_len", &benchmark_name),
            &benchmark_name,
            |b, _| b.iter(|| nybbles1.eq(&nybbles2)),
        );

        // Not equal comparison for PackedNibbles (different lengths)
        group.bench_with_input(
            BenchmarkId::new("PackedNibbles_diff_len", &benchmark_name),
            &benchmark_name,
            |b, _| b.iter(|| packed1.eq(&packed2)),
        );
    }

    group.finish();
}

fn generate_prefixed_nibbles(
    rng: &mut impl Rng,
    size: usize,
    prefix_len: usize,
) -> (Vec<u8>, Vec<u8>) {
    // Generate common prefix
    let prefix: Vec<u8> = (0..prefix_len).map(|_| rng.random_range(0..16)).collect();

    // Generate first set with prefix + random suffix
    let mut nibbles1 = prefix.clone();
    nibbles1.extend((0..(size - prefix_len)).map(|_| rng.random_range(0..16)));

    // Generate second set with same prefix but different suffix
    let mut nibbles2 = prefix;
    nibbles2.extend((0..(size - prefix_len)).map(|_| rng.random_range(0..16)));

    (nibbles1, nibbles2)
}

fn bench_common_prefixes(c: &mut Criterion) {
    let mut rng = rand::rng();
    let mut group = c.benchmark_group("eq_with_common_prefix");

    // Test nibbles with increasing common prefix lengths
    for size in [32, 64] {
        for prefix_percent in [25, 50, 75, 90] {
            let prefix_len = (size * prefix_percent) / 100;

            // Generate two sets of nibbles with common prefix
            let (nibbles1, nibbles2) = generate_prefixed_nibbles(&mut rng, size, prefix_len);

            // Create instances
            let nybbles1 = Nibbles::from_nibbles_unchecked(nibbles1.clone());
            let packed1 = PackedNibbles::from_nibbles(nibbles1);

            let nybbles2 = Nibbles::from_nibbles_unchecked(nibbles2.clone());
            let packed2 = PackedNibbles::from_nibbles(nibbles2);

            let benchmark_name = format!("size_{}_prefix_{}%", size, prefix_percent);

            // Benchmark equality comparison with common prefix
            group.bench_with_input(
                BenchmarkId::new("Nibbles_common_prefix", &benchmark_name),
                &benchmark_name,
                |b, _| b.iter(|| nybbles1.eq(&nybbles2)),
            );

            group.bench_with_input(
                BenchmarkId::new("PackedNibbles_common_prefix", &benchmark_name),
                &benchmark_name,
                |b, _| b.iter(|| packed1.eq(&packed2)),
            );
        }
    }

    group.finish();
}

fn bench_clone(c: &mut Criterion) {
    let mut rng = rand::rng();
    let mut group = c.benchmark_group("clone");

    // Test with different nibble lengths
    for size in [4, 8, 16, 32, 64] {
        // Generate test data for each size
        let (nybbles, packed) = generate_nibbles(&mut rng, size);

        // Benchmark cloning for Nibbles
        group.bench_with_input(BenchmarkId::new("Nibbles", size), &size, |b, _| {
            b.iter(|| nybbles.clone())
        });

        // Benchmark cloning for PackedNibbles
        group.bench_with_input(BenchmarkId::new("PackedNibbles", size), &size, |b, _| {
            b.iter(|| packed.clone())
        });
    }

    group.finish();
}

criterion_group!(benches, bench_eq_same_len, bench_eq_diff_len, bench_common_prefixes, bench_clone);
criterion_main!(benches);

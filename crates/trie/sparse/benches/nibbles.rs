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

    let mut group = c.benchmark_group("eq_comparison_same_len_eq");
    for size in [4, 8, 16, 32, 64] {
        let (nybbles1, packed1) = generate_nibbles(&mut rng, size);

        let nybbles1_clone = nybbles1.clone();
        let packed1_clone = packed1.clone();

        group.bench_with_input(BenchmarkId::new("Nibbles", size), &size, |b, _| {
            b.iter(|| nybbles1.eq(&nybbles1_clone))
        });
        group.bench_with_input(BenchmarkId::new("PackedNibbles", size), &size, |b, _| {
            b.iter(|| packed1.eq(&packed1_clone))
        });
    }
    group.finish();

    let mut group = c.benchmark_group("eq_comparison_same_len_not_eq");
    for size in [4, 8, 16, 32, 64] {
        let (nybbles1, packed1) = generate_nibbles(&mut rng, size);
        let (nybbles2, packed2) = generate_nibbles(&mut rng, size);

        group.bench_with_input(BenchmarkId::new("Nibbles", size), &size, |b, _| {
            b.iter(|| nybbles1.eq(&nybbles2))
        });
        group.bench_with_input(BenchmarkId::new("PackedNibbles", size), &size, |b, _| {
            b.iter(|| packed1.eq(&packed2))
        });
    }
    group.finish();
}

fn bench_eq_diff_len(c: &mut Criterion) {
    let mut rng = rand::rng();

    let mut group = c.benchmark_group("eq_comparison_diff_len");
    for (len1, len2) in [(4, 8), (8, 16), (16, 32), (32, 64)] {
        let (nybbles1, packed1) = generate_nibbles(&mut rng, len1);
        let (nybbles2, packed2) = generate_nibbles(&mut rng, len2);

        group.bench_with_input(BenchmarkId::new("Nibbles", len1), &len1, |b, _| {
            b.iter(|| nybbles1.eq(&nybbles2))
        });
        group.bench_with_input(BenchmarkId::new("PackedNibbles", len1), &len1, |b, _| {
            b.iter(|| packed1.eq(&packed2))
        });
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

    for size in [32, 64] {
        let mut group = c.benchmark_group(format!("eq_with_common_prefix_size_{}", size));
        for prefix_percent in [25, 50, 75, 90] {
            let prefix_len = (size * prefix_percent) / 100;

            let (nibbles1, nibbles2) = generate_prefixed_nibbles(&mut rng, size, prefix_len);

            let nybbles1 = Nibbles::from_nibbles_unchecked(nibbles1.clone());
            let packed1 = PackedNibbles::from_nibbles(nibbles1);

            let nybbles2 = Nibbles::from_nibbles_unchecked(nibbles2.clone());
            let packed2 = PackedNibbles::from_nibbles(nibbles2);

            group.bench_with_input(
                BenchmarkId::new("Nibbles", prefix_percent),
                &prefix_percent,
                |b, _| b.iter(|| nybbles1.eq(&nybbles2)),
            );
            group.bench_with_input(
                BenchmarkId::new("PackedNibbles", prefix_percent),
                &prefix_percent,
                |b, _| b.iter(|| packed1.eq(&packed2)),
            );
        }
        group.finish();
    }
}

fn bench_clone(c: &mut Criterion) {
    let mut rng = rand::rng();

    let mut group = c.benchmark_group("clone");
    for size in [4, 8, 16, 32, 64] {
        let (nybbles, packed) = generate_nibbles(&mut rng, size);

        group.bench_with_input(BenchmarkId::new("Nibbles", size), &size, |b, _| {
            b.iter(|| nybbles.clone())
        });
        group.bench_with_input(BenchmarkId::new("PackedNibbles", size), &size, |b, _| {
            b.iter(|| packed.clone())
        });
    }
    group.finish();
}

fn bench_slice(c: &mut Criterion) {
    let mut rng = rand::rng();

    let mut group = c.benchmark_group("slice");
    for size in [16, 32, 64] {
        // Generate random nibbles
        let (nybbles, packed_nibbles) = generate_nibbles(&mut rng, size);

        // Slice middle 25%
        let start = size / 4;
        let end = start + (size / 2);

        group.bench_with_input(BenchmarkId::new("Nibbles", size), &size, |b, _| {
            b.iter(|| nybbles.slice(start..end))
        });
        group.bench_with_input(BenchmarkId::new("PackedNibbles", size), &size, |b, _| {
            b.iter(|| packed_nibbles.slice(start..end))
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_eq_same_len,
    bench_eq_diff_len,
    bench_common_prefixes,
    bench_clone,
    bench_slice
);
criterion_main!(benches);

#![allow(missing_docs)]

use criterion::{black_box, criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion};
use nybbles::Nibbles;
use rand::{distr::Uniform, rngs::ThreadRng, Rng};
use reth_trie_sparse::PackedNibbles;

fn generate_nibbles(rng: &mut impl Rng, length: usize) -> (Nibbles, PackedNibbles) {
    // Generate random nibbles
    let nibbles: Vec<u8> = rng.sample_iter(Uniform::new(0, 16).unwrap()).take(length).collect();

    // Create instances of both types with same values
    let nybbles_nibbles = Nibbles::from_nibbles_unchecked(nibbles.clone());
    let packed_nibbles = PackedNibbles::from_nibbles(nibbles);

    (nybbles_nibbles, packed_nibbles)
}

fn generate_prefixed_nibbles(
    rng: &mut impl Rng,
    length: usize,
    prefix_len: usize,
) -> (Vec<u8>, Vec<u8>) {
    // Generate common prefix
    let prefix: Vec<u8> = (0..prefix_len).map(|_| rng.random_range(0..16)).collect();

    // Generate first set with prefix + random suffix
    let mut nibbles1 = prefix.clone();
    nibbles1.extend((0..(length - prefix_len)).map(|_| rng.random_range(0..16)));

    // Generate second set with same prefix but different suffix
    let mut nibbles2 = prefix;
    nibbles2.extend((0..(length - prefix_len)).map(|_| rng.random_range(0..16)));

    (nibbles1, nibbles2)
}

fn bench_push_unchecked(c: &mut Criterion) {
    let mut rng = rand::rng();
    let nibble_range = Uniform::new(0, 16).unwrap();

    let nibbles = |rng: &mut ThreadRng| {
        // Always leave space for at least one more nibble to be pushed
        let length = rng.random_range(0..63);
        rng.sample_iter(nibble_range).take(length).collect::<Vec<_>>()
    };

    let mut group = c.benchmark_group("push_unchecked");
    group.bench_function("Nibbles", |b| {
        b.iter_batched(
            || (Nibbles::from_nibbles_unchecked(nibbles(&mut rng)), rng.random_range(0..16)),
            |(nibbles, nibble)| black_box(nibbles).push_unchecked(nibble),
            BatchSize::SmallInput,
        )
    });
    group.bench_function("PackedNibbles", |b| {
        b.iter_batched(
            || (PackedNibbles::from_nibbles_unchecked(nibbles(&mut rng)), rng.random_range(0..16)),
            |(nibbles, nibble)| black_box(nibbles).push_unchecked(nibble),
            BatchSize::SmallInput,
        )
    });
    group.finish();
}

fn bench_eq(c: &mut Criterion) {
    let mut rng = rand::rng();

    let (nybbles1, packed1) = generate_nibbles(&mut rng, 64);

    let nybbles1_clone = nybbles1.clone();
    let packed1_clone = packed1;

    let mut group = c.benchmark_group("eq");
    group.bench_function("Nibbles", |b| {
        b.iter(|| black_box(&nybbles1).eq(black_box(&nybbles1_clone)))
    });
    group.bench_function("PackedNibbles", |b| {
        b.iter(|| black_box(&packed1).eq(black_box(&packed1_clone)))
    });
    group.finish();
}

fn bench_common_prefix_length(c: &mut Criterion) {
    let mut rng = rand::rng();

    let mut group = c.benchmark_group("common_prefix");
    for prefix_percent in [25, 50, 75, 90] {
        let prefix_len = (64 * prefix_percent) / 100;

        let (nibbles1, nibbles2) = generate_prefixed_nibbles(&mut rng, 64, prefix_len);

        let nybbles1 = Nibbles::from_nibbles_unchecked(nibbles1.clone());
        let packed1 = PackedNibbles::from_nibbles(nibbles1);

        let nybbles2 = Nibbles::from_nibbles_unchecked(nibbles2.clone());
        let packed2 = PackedNibbles::from_nibbles(nibbles2);

        group.bench_with_input(
            BenchmarkId::new("Nibbles", prefix_percent),
            &prefix_percent,
            |b, _| b.iter(|| black_box(&nybbles1).common_prefix_length(black_box(&nybbles2))),
        );
        group.bench_with_input(
            BenchmarkId::new("PackedNibbles", prefix_percent),
            &prefix_percent,
            |b, _| b.iter(|| black_box(&packed1).common_prefix_length(black_box(&packed2))),
        );
    }
    group.finish();
}

fn bench_clone(c: &mut Criterion) {
    let mut rng = rand::rng();

    let (nybbles, packed) = generate_nibbles(&mut rng, 64);

    let mut group = c.benchmark_group("clone");
    group.bench_function("Nibbles", |b| b.iter(|| black_box(&nybbles).clone()));
    group.bench_function("PackedNibbles", |b| b.iter(|| *black_box(&packed)));
    group.finish();
}

fn bench_slice(c: &mut Criterion) {
    let mut rng = rand::rng();

    let (nybbles, packed_nibbles) = generate_nibbles(&mut rng, 64);
    // Slice middle 25%
    let start = 64 / 4;
    let end = start + (64 / 2);

    let mut group = c.benchmark_group("slice_even");
    group
        .bench_function("Nibbles", |b| b.iter(|| black_box(&nybbles).slice(black_box(start..end))));
    group.bench_function("PackedNibbles", |b| {
        b.iter(|| black_box(&packed_nibbles).slice(black_box(start..end)))
    });
    group.finish();

    let (nybbles, packed_nibbles) = generate_nibbles(&mut rng, 64);
    // Slice middle 25%
    let start = 64 / 4 + 1;
    let end = start + (64 / 2) + 1;

    let mut group = c.benchmark_group("slice_odd");
    group
        .bench_function("Nibbles", |b| b.iter(|| black_box(&nybbles).slice(black_box(start..end))));
    group.bench_function("PackedNibbles", |b| {
        b.iter(|| black_box(&packed_nibbles).slice(black_box(start..end)))
    });
    group.finish();
}

fn bench_starts_with(c: &mut Criterion) {
    let mut rng = rand::rng();
    let total_len = 64;

    let mut group = c.benchmark_group("starts_with");
    for prefix_percent in [25, 50, 75] {
        let prefix_len = (total_len * prefix_percent) / 100;

        // Generate completely different nibbles that will return `false` for the prefix
        let (nybbles, packed) = generate_nibbles(&mut rng, total_len);
        let (nybbles_prefix, packed_prefix) = generate_nibbles(&mut rng, prefix_len);

        group.bench_with_input(
            BenchmarkId::new("Nibbles", prefix_percent),
            &prefix_percent,
            |b, _| b.iter(|| black_box(&nybbles).starts_with(black_box(&nybbles_prefix))),
        );
        group.bench_with_input(
            BenchmarkId::new("PackedNibbles", prefix_percent),
            &prefix_percent,
            |b, _| b.iter(|| black_box(&packed).starts_with(black_box(&packed_prefix))),
        );
    }
    group.finish();
}

fn bench_ord(c: &mut Criterion) {
    let mut rng = rand::rng();

    let mut group = c.benchmark_group("ord");
    for prefix_percent in [50, 75, 90] {
        let prefix_len = (64 * prefix_percent) / 100;

        let (nibbles1, nibbles2) = generate_prefixed_nibbles(&mut rng, 64, prefix_len);

        let nybbles1 = Nibbles::from_nibbles_unchecked(nibbles1.clone());
        let packed1 = PackedNibbles::from_nibbles(nibbles1);

        let nybbles2 = Nibbles::from_nibbles_unchecked(nibbles2.clone());
        let packed2 = PackedNibbles::from_nibbles(nibbles2);

        group.bench_with_input(
            BenchmarkId::new("Nibbles", prefix_percent),
            &prefix_percent,
            |b, _| b.iter(|| black_box(&nybbles1).partial_cmp(black_box(&nybbles2))),
        );
        group.bench_with_input(
            BenchmarkId::new("PackedNibbles", prefix_percent),
            &prefix_percent,
            |b, _| b.iter(|| black_box(&packed1).partial_cmp(black_box(&packed2))),
        );
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_push_unchecked,
    bench_eq,
    bench_common_prefix_length,
    bench_clone,
    bench_slice,
    bench_starts_with,
    bench_ord,
);
criterion_main!(benches);

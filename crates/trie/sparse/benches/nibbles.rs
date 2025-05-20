#![allow(missing_docs)]

use criterion::{criterion_group, criterion_main, Bencher, BenchmarkId, Criterion};
use nybbles::Nibbles;
use rand::Rng;
use reth_trie_sparse::PackedNibbles;

fn generate_nibbles(rng: &mut impl Rng, length: usize) -> (Nibbles, PackedNibbles) {
    // Generate random nibbles
    let nibbles: Vec<u8> = (0..length).map(|_| rng.random_range(0..16)).collect();

    // Create instances of both types with same values
    let nybbles_nibbles = Nibbles::from_nibbles_unchecked(nibbles.clone());
    assert_eq!(nybbles_nibbles.as_slice().len(), length);
    let packed_nibbles = PackedNibbles::from_nibbles(nibbles);
    assert_eq!(packed_nibbles.as_slice().len(), length / 2);

    (nybbles_nibbles, packed_nibbles)
}

fn bench_eq(c: &mut Criterion) {
    let mut rng = rand::rng();

    let mut group = c.benchmark_group("eq");
    for size in [32, 64] {
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

fn bench_common_prefix_length(c: &mut Criterion) {
    let mut rng = rand::rng();

    for size in [32, 64] {
        let mut group = c.benchmark_group(format!("common_prefix_{size}"));
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
                |b, _| b.iter(|| nybbles1.common_prefix_length(&nybbles2)),
            );
            group.bench_with_input(
                BenchmarkId::new("PackedNibbles", prefix_percent),
                &prefix_percent,
                |b, _| b.iter(|| packed1.common_prefix_length(&packed2)),
            );
        }
        group.finish();
    }
}

fn bench_clone(c: &mut Criterion) {
    let mut rng = rand::rng();

    let mut group = c.benchmark_group("clone");
    for size in [16, 32, 64] {
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

    let mut group = c.benchmark_group("slice_even");
    for size in [32, 64] {
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

    let mut group = c.benchmark_group("slice_odd");
    for size in [32, 64] {
        let (nybbles, packed_nibbles) = generate_nibbles(&mut rng, size);

        // Slice middle 25%
        let start = size / 4 + 1;
        let end = start + (size / 2) + 1;

        group.bench_with_input(BenchmarkId::new("Nibbles", size), &size, |b, _| {
            b.iter(|| nybbles.slice(start..end))
        });
        group.bench_with_input(BenchmarkId::new("PackedNibbles", size), &size, |b, _| {
            b.iter(|| packed_nibbles.slice(start..end))
        });
    }
    group.finish();
}

fn bench_starts_with(c: &mut Criterion) {
    let mut rng = rand::rng();

    let mut group = c.benchmark_group("starts_with_true");
    for size in [32, 64] {
        let prefix_len = 16;

        let (full_nybbles, full_packed) = generate_nibbles(&mut rng, size);

        let nybbles_prefix = full_nybbles.slice(0..prefix_len);
        let packed_prefix = full_packed.slice(0..prefix_len);

        group.bench_with_input(BenchmarkId::new("Nibbles", size), &size, |b, _| {
            b.iter(|| full_nybbles.starts_with(&nybbles_prefix))
        });
        group.bench_with_input(BenchmarkId::new("PackedNibbles", size), &size, |b, _| {
            b.iter(|| full_packed.starts_with(&packed_prefix))
        });
    }
    group.finish();

    let mut group = c.benchmark_group("starts_with_false");
    for size in [32, 64] {
        let prefix_len = 16;

        let (nybbles1, packed1) = generate_nibbles(&mut rng, size);
        let (nybbles2, packed2) = generate_nibbles(&mut rng, prefix_len);

        group.bench_with_input(BenchmarkId::new("Nibbles", size), &size, |b, _| {
            b.iter(|| nybbles1.starts_with(&nybbles2))
        });
        group.bench_with_input(BenchmarkId::new("PackedNibbles", size), &size, |b, _| {
            b.iter(|| packed1.starts_with(&packed2))
        });
    }
    group.finish();
}

fn bench_ord(c: &mut Criterion) {
    let mut rng = rand::rng();

    for size in [32, 64] {
        let mut group = c.benchmark_group(format!("ord_{size}"));
        for prefix_percent in [50, 75, 90] {
            let prefix_len = (size * prefix_percent) / 100;

            let (nibbles1, nibbles2) = generate_prefixed_nibbles(&mut rng, size, prefix_len);

            let nybbles1 = Nibbles::from_nibbles_unchecked(nibbles1.clone());
            let packed1 = PackedNibbles::from_nibbles(nibbles1);

            let nybbles2 = Nibbles::from_nibbles_unchecked(nibbles2.clone());
            let packed2 = PackedNibbles::from_nibbles(nibbles2);

            group.bench_with_input(
                BenchmarkId::new("Nibbles", prefix_percent),
                &prefix_percent,
                |b, _| b.iter(|| nybbles1.partial_cmp(&nybbles2)),
            );
            group.bench_with_input(
                BenchmarkId::new("PackedNibbles", prefix_percent),
                &prefix_percent,
                |b, _| b.iter(|| packed1.partial_cmp(&packed2)),
            );
        }
        group.finish();
    }
}

fn bench_clone_2(c: &mut Criterion) {
    let mut group = c.benchmark_group("clone_2");

    group.bench_function(BenchmarkId::new("smallvec", 32), |b| {
        struct SmallVecData(smallvec::SmallVec<[u8; 32]>);

        impl Clone for SmallVecData {
            #[inline(never)]
            fn clone(&self) -> Self {
                Self(smallvec::SmallVec::from_slice(&self.0))
            }
        }

        let data = SmallVecData(smallvec::smallvec![1; 32]);
        b.iter(|| data.clone())
    });

    group.bench_function(BenchmarkId::new("smallvec", 64), |b| {
        struct SmallVecData(smallvec::SmallVec<[u8; 64]>);

        impl Clone for SmallVecData {
            #[inline(never)]
            fn clone(&self) -> Self {
                Self(smallvec::SmallVec::from_slice(&self.0))
            }
        }

        let data = SmallVecData(smallvec::smallvec![1; 64]);
        b.iter(|| data.clone())
    });

    group.bench_function(BenchmarkId::new("tinyvec", 32), |b| {
        struct TinyVecData(tinyvec::ArrayVec<[u8; 32]>);

        impl Clone for TinyVecData {
            #[inline(never)]
            fn clone(&self) -> Self {
                Self(self.0)
            }
        }

        let data = TinyVecData(tinyvec::array_vec![1; 32]);
        b.iter(|| data.clone());
    });

    group.bench_function(BenchmarkId::new("tinyvec", 64), |b| {
        struct TinyVecData(tinyvec::ArrayVec<[u8; 64]>);

        impl Clone for TinyVecData {
            #[inline(never)]
            fn clone(&self) -> Self {
                Self(self.0)
            }
        }

        let data = TinyVecData(tinyvec::array_vec![1; 64]);
        b.iter(|| data.clone());
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_eq,
    bench_common_prefix_length,
    bench_clone,
    bench_slice,
    bench_starts_with,
    bench_ord,
    bench_clone_2,
);
criterion_main!(benches);

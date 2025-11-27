#![allow(missing_docs, unreachable_pub)]
use criterion::{
    black_box, criterion_group, criterion_main, BatchSize, BenchmarkGroup, BenchmarkId, Criterion,
};
use reth_trie_common::utils::{extend_sorted_vec, extend_sorted_vec_new};

/// Generate a sorted vector of (u64, u64) tuples
fn generate_sorted_vec(size: usize, start: u64) -> Vec<(u64, u64)> {
    (start..start + size as u64).map(|i| (i, i * 2)).collect()
}

/// Generate two overlapping sorted vectors with some common keys
fn generate_overlapping_vecs(
    target_size: usize,
    other_size: usize,
) -> (Vec<(u64, u64)>, Vec<(u64, u64)>) {
    // Target vec starts at 0
    let target = generate_sorted_vec(target_size, 0);

    // Other vec starts at target_size/2, creating 50% overlap
    let other = generate_sorted_vec(other_size, (target_size / 2) as u64);

    (target, other)
}

fn bench_extend_sorted_vec_old(
    group: &mut BenchmarkGroup<'_, criterion::measurement::WallTime>,
    target_size: usize,
    other_size: usize,
) {
    let id = BenchmarkId::new("extend_sorted_vec", format!("t{}_o{}", target_size, other_size));

    group.bench_function(id, |b| {
        b.iter_batched_ref(
            || generate_overlapping_vecs(target_size, other_size),
            |(target, other)| {
                extend_sorted_vec(black_box(target), black_box(other));
            },
            BatchSize::SmallInput,
        );
    });
}

fn bench_extend_sorted_vec_new(
    group: &mut BenchmarkGroup<'_, criterion::measurement::WallTime>,
    target_size: usize,
    other_size: usize,
) {
    let id = BenchmarkId::new("extend_sorted_vec_new", format!("t{}_o{}", target_size, other_size));

    group.bench_function(id, |b| {
        b.iter_batched_ref(
            || generate_overlapping_vecs(target_size, other_size),
            |(target, other)| {
                extend_sorted_vec_new(black_box(target), black_box(other));
            },
            BatchSize::SmallInput,
        );
    });
}

fn extend_sorted_vec_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("extend_sorted_vec_comparison");

    // Test different combinations of target size and other size
    let target_sizes = vec![10, 100, 1000, 5000];
    let other_sizes = vec![10, 100, 1000, 5000];

    for &target_size in &target_sizes {
        for &other_size in &other_sizes {
            bench_extend_sorted_vec_old(&mut group, target_size, other_size);
            bench_extend_sorted_vec_new(&mut group, target_size, other_size);
        }
    }

    group.finish();
}

criterion_group!(benches, extend_sorted_vec_benchmark);
criterion_main!(benches);

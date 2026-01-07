#![allow(missing_docs, unreachable_pub)]

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use proptest::{prelude::*, strategy::ValueTree, test_runner::TestRunner};

/// Old implementation: iterate + peek + collect + sort (the actual previous code)
fn extend_sorted_vec_old<K, V>(target: &mut Vec<(K, V)>, other: &[(K, V)])
where
    K: Clone + Ord,
    V: Clone,
{
    use core::cmp::Ordering;

    if other.is_empty() {
        return;
    }

    let mut other_iter = other.iter().peekable();
    let mut to_insert = Vec::new();

    for target_item in target.iter_mut() {
        while let Some(other_item) = other_iter.peek() {
            match other_item.0.cmp(&target_item.0) {
                Ordering::Less => {
                    to_insert.push(other_iter.next().unwrap().clone());
                }
                Ordering::Equal => {
                    target_item.1 = other_iter.next().unwrap().1.clone();
                    break;
                }
                Ordering::Greater => {
                    break;
                }
            }
        }
    }

    if !to_insert.is_empty() || other_iter.peek().is_some() {
        target.extend(to_insert);
        target.extend(other_iter.cloned());
        target.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    }
}

/// New implementation: two-pointer merge (O(n+m))
fn extend_sorted_vec_new<K, V>(target: &mut Vec<(K, V)>, other: &[(K, V)])
where
    K: Clone + Ord,
    V: Clone,
{
    use core::cmp::Ordering;

    if other.is_empty() {
        return;
    }
    if target.is_empty() {
        target.extend(other.iter().cloned());
        dedup_sorted_by_key(target);
        return;
    }

    let mut merged = Vec::with_capacity(target.len() + other.len());

    let mut target_idx = 0;
    let mut other_idx = 0;

    while target_idx < target.len() && other_idx < other.len() {
        let target_key = &target[target_idx].0;
        let other_key = &other[other_idx].0;

        match target_key.cmp(other_key) {
            Ordering::Less => {
                push_or_update(&mut merged, &target[target_idx]);
                target_idx += 1;
            }
            Ordering::Greater => {
                push_or_update(&mut merged, &other[other_idx]);
                other_idx += 1;
            }
            Ordering::Equal => {
                push_or_update(&mut merged, &target[target_idx]);
                target_idx += 1;
            }
        }
    }

    while target_idx < target.len() {
        push_or_update(&mut merged, &target[target_idx]);
        target_idx += 1;
    }

    while other_idx < other.len() {
        push_or_update(&mut merged, &other[other_idx]);
        other_idx += 1;
    }

    *target = merged;
}

#[inline]
fn push_or_update<K: Clone + Eq, V: Clone>(merged: &mut Vec<(K, V)>, entry: &(K, V)) {
    if let Some((last_key, last_value)) = merged.last_mut() {
        if *last_key == entry.0 {
            *last_value = entry.1.clone();
            return;
        }
    }
    merged.push((entry.0.clone(), entry.1.clone()));
}

#[inline]
fn dedup_sorted_by_key<K: Eq, V>(vec: &mut Vec<(K, V)>) {
    if vec.len() <= 1 {
        return;
    }

    let mut write_idx = 0;
    for read_idx in 1..vec.len() {
        if vec[write_idx].0 == vec[read_idx].0 {
            vec.swap(write_idx, read_idx);
        } else {
            write_idx += 1;
            if write_idx != read_idx {
                vec.swap(write_idx, read_idx);
            }
        }
    }
    vec.truncate(write_idx + 1);
}

fn generate_sorted_data(size: usize, offset: u64) -> Vec<(u64, u64)> {
    let mut runner = TestRunner::deterministic();

    let mut vec: Vec<(u64, u64)> =
        prop::collection::vec((offset..offset + 1_000_000, any::<u64>()), size)
            .new_tree(&mut runner)
            .unwrap()
            .current();
    vec.sort_by_key(|x| x.0);
    vec.dedup_by_key(|x| x.0);
    vec
}

fn bench_extend_sorted_vec(c: &mut Criterion) {
    let mut group = c.benchmark_group("extend_sorted_vec");

    for target_size in [100, 1_000, 10_000, 100_000] {
        #[expect(unexpected_cfgs)]
        if cfg!(codspeed) && target_size > 10_000 {
            continue;
        }

        let other_size = 100;

        group.throughput(Throughput::Elements((target_size + other_size) as u64));

        group.bench_function(
            BenchmarkId::new("old_sort", format!("target_{}_other_{}", target_size, other_size)),
            |b| {
                b.iter_with_setup(
                    || {
                        let target = generate_sorted_data(target_size, 0);
                        let other = generate_sorted_data(other_size, 500_000);
                        (target, other)
                    },
                    |(mut target, other)| {
                        extend_sorted_vec_old(&mut target, &other);
                        black_box(target)
                    },
                );
            },
        );

        group.bench_function(
            BenchmarkId::new("new_merge", format!("target_{}_other_{}", target_size, other_size)),
            |b| {
                b.iter_with_setup(
                    || {
                        let target = generate_sorted_data(target_size, 0);
                        let other = generate_sorted_data(other_size, 500_000);
                        (target, other)
                    },
                    |(mut target, other)| {
                        extend_sorted_vec_new(&mut target, &other);
                        black_box(target)
                    },
                );
            },
        );
    }

    // Test with overlapping keys
    group.bench_function(BenchmarkId::new("old_sort", "overlapping_10k_100"), |b| {
        b.iter_with_setup(
            || {
                let target = generate_sorted_data(10_000, 0);
                let other = generate_sorted_data(100, 0);
                (target, other)
            },
            |(mut target, other)| {
                extend_sorted_vec_old(&mut target, &other);
                black_box(target)
            },
        );
    });

    group.bench_function(BenchmarkId::new("new_merge", "overlapping_10k_100"), |b| {
        b.iter_with_setup(
            || {
                let target = generate_sorted_data(10_000, 0);
                let other = generate_sorted_data(100, 0);
                (target, other)
            },
            |(mut target, other)| {
                extend_sorted_vec_new(&mut target, &other);
                black_box(target)
            },
        );
    });

    group.finish();
}

fn bench_cumulative_extend(c: &mut Criterion) {
    let mut group = c.benchmark_group("cumulative_extend");

    for num_blocks in [10, 50, 100] {
        #[expect(unexpected_cfgs)]
        if cfg!(codspeed) && num_blocks > 50 {
            continue;
        }

        let changes_per_block = 100;

        group.bench_function(BenchmarkId::new("old_sort", format!("{}_blocks", num_blocks)), |b| {
            b.iter_with_setup(
                || {
                    (0..num_blocks)
                        .map(|i| generate_sorted_data(changes_per_block, i as u64 * 10_000))
                        .collect::<Vec<_>>()
                },
                |block_deltas| {
                    let mut overlay: Vec<(u64, u64)> = Vec::new();
                    for delta in &block_deltas {
                        extend_sorted_vec_old(&mut overlay, delta);
                    }
                    black_box(overlay)
                },
            );
        });

        group.bench_function(
            BenchmarkId::new("new_merge", format!("{}_blocks", num_blocks)),
            |b| {
                b.iter_with_setup(
                    || {
                        (0..num_blocks)
                            .map(|i| generate_sorted_data(changes_per_block, i as u64 * 10_000))
                            .collect::<Vec<_>>()
                    },
                    |block_deltas| {
                        let mut overlay: Vec<(u64, u64)> = Vec::new();
                        for delta in &block_deltas {
                            extend_sorted_vec_new(&mut overlay, delta);
                        }
                        black_box(overlay)
                    },
                );
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_extend_sorted_vec, bench_cumulative_extend);
criterion_main!(benches);

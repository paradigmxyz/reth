//! Nibbles benchmarks

#![allow(missing_docs)]

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use nybbles::Nibbles;

/// Prepend a single nibble to an existing Nibbles path.
/// This is a proposed optimization for the common trie operation.
#[inline]
fn prepend_nibble_unchecked(nibble: u8, suffix: &Nibbles) -> Nibbles {
    let mut result = Nibbles::from_nibbles_unchecked([nibble]);
    result.extend(suffix);
    result
}

/// Benchmark creating single-nibble Nibbles repeatedly (current approach)
fn bench_single_nibble_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("nibbles_creation");

    group.bench_function("single_nibble_from_array", |b| {
        b.iter(|| {
            for nibble in 0u8..16 {
                let n = Nibbles::from_nibbles_unchecked([nibble]);
                black_box(n);
            }
        });
    });

    group.bench_function("single_nibble_from_slice", |b| {
        b.iter(|| {
            for nibble in 0u8..16 {
                let slice: &[u8] = &[nibble];
                let n = Nibbles::from_nibbles_unchecked(slice);
                black_box(n);
            }
        });
    });

    group.finish();
}

/// Benchmark prepending a nibble to an existing path (common trie operation)
fn bench_prepend_nibble(c: &mut Criterion) {
    let mut group = c.benchmark_group("nibbles_prepend");

    // Create a typical extension key (4 nibbles)
    let existing_key = Nibbles::from_nibbles_unchecked([0x1, 0x2, 0x3, 0x4]);

    group.bench_function("prepend_via_new_nibbles_extend", |b| {
        b.iter(|| {
            for child_nibble in 0u8..16 {
                let mut new_key = Nibbles::from_nibbles_unchecked([child_nibble]);
                new_key.extend(&existing_key);
                black_box(new_key);
            }
        });
    });

    // Alternative: use cached single nibbles
    group.bench_function("prepend_via_cached_clone_extend", |b| {
        // Pre-create cached nibbles
        let cached: [Nibbles; 16] =
            core::array::from_fn(|i| Nibbles::from_nibbles_unchecked([i as u8]));

        b.iter(|| {
            for child_nibble in 0u8..16 {
                let mut new_key = cached[child_nibble as usize];
                new_key.extend(&existing_key);
                black_box(new_key);
            }
        });
    });

    group.finish();
}

/// Benchmark full trie path construction patterns
fn bench_trie_path_patterns(c: &mut Criterion) {
    let mut group = c.benchmark_group("trie_path_patterns");

    // Simulate branch node downgrade: creating extension from single nibble
    group.bench_function("branch_to_extension_single_nibble", |b| {
        b.iter(|| {
            for child_nibble in 0u8..16 {
                let ext = Nibbles::from_nibbles_unchecked([child_nibble]);
                black_box(ext);
            }
        });
    });

    // Simulate extension merge: prepend nibble to existing extension key
    let ext_key = Nibbles::from_nibbles_unchecked([0x5, 0x6, 0x7, 0x8, 0x9, 0xA]);
    group.bench_function("extension_prepend_nibble", |b| {
        b.iter(|| {
            for child_nibble in 0u8..16 {
                let mut new_key = Nibbles::from_nibbles_unchecked([child_nibble]);
                new_key.extend(&ext_key);
                black_box(new_key);
            }
        });
    });

    // Simulate leaf merge: prepend nibble to leaf key
    let leaf_key = Nibbles::from_nibbles_unchecked([
        0x1, 0x2, 0x3, 0x4, 0x5, 0x6, 0x7, 0x8, 0x9, 0xA, 0xB, 0xC, 0xD, 0xE, 0xF, 0x0,
    ]);
    group.bench_function("leaf_prepend_nibble", |b| {
        b.iter(|| {
            for child_nibble in 0u8..16 {
                let mut new_key = Nibbles::from_nibbles_unchecked([child_nibble]);
                new_key.extend(&leaf_key);
                black_box(new_key);
            }
        });
    });

    group.finish();
}

/// Benchmark batch operations that simulate real trie updates
fn bench_batch_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("batch_operations");

    // Simulate 1000 branch downgrades (common during trie pruning)
    group.bench_function("1000_branch_downgrades_current", |b| {
        let keys: Vec<Nibbles> = (0..1000)
            .map(|i| {
                Nibbles::from_nibbles_unchecked([
                    (i % 16) as u8,
                    ((i / 16) % 16) as u8,
                    ((i / 256) % 16) as u8,
                ])
            })
            .collect();

        b.iter(|| {
            for (i, key) in keys.iter().enumerate() {
                let child_nibble = (i % 16) as u8;
                let mut new_key = Nibbles::from_nibbles_unchecked([child_nibble]);
                new_key.extend(key);
                black_box(new_key);
            }
        });
    });

    // With pre-cached single nibbles
    group.bench_function("1000_branch_downgrades_cached", |b| {
        let cached: [Nibbles; 16] =
            core::array::from_fn(|i| Nibbles::from_nibbles_unchecked([i as u8]));
        let keys: Vec<Nibbles> = (0..1000)
            .map(|i| {
                Nibbles::from_nibbles_unchecked([
                    (i % 16) as u8,
                    ((i / 16) % 16) as u8,
                    ((i / 256) % 16) as u8,
                ])
            })
            .collect();

        b.iter(|| {
            for (i, key) in keys.iter().enumerate() {
                let child_nibble = (i % 16) as u8;
                let mut new_key = cached[child_nibble as usize];
                new_key.extend(key);
                black_box(new_key);
            }
        });
    });

    // Alternative: prepend using a helper that directly constructs
    group.bench_function("1000_branch_downgrades_prepend_helper", |b| {
        let keys: Vec<Nibbles> = (0..1000)
            .map(|i| {
                Nibbles::from_nibbles_unchecked([
                    (i % 16) as u8,
                    ((i / 16) % 16) as u8,
                    ((i / 256) % 16) as u8,
                ])
            })
            .collect();

        b.iter(|| {
            for (i, key) in keys.iter().enumerate() {
                let child_nibble = (i % 16) as u8;
                // Use prepend_nibble_unchecked helper
                let new_key = prepend_nibble_unchecked(child_nibble, key);
                black_box(new_key);
            }
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_single_nibble_creation,
    bench_prepend_nibble,
    bench_trie_path_patterns,
    bench_batch_operations,
);
criterion_main!(benches);

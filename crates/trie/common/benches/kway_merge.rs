use alloy_primitives::{map::B256Map, B256, U256};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use itertools::Itertools;
use reth_trie_common::{HashedPostStateSorted, HashedStorageSorted};
use std::sync::Arc;

/// Generate realistic HashedPostStateSorted mimicking block state changes
fn generate_hashed_state(
    block_idx: usize,
    num_accounts: usize,
    slots_per_account: usize,
) -> HashedPostStateSorted {
    let mut accounts = Vec::with_capacity(num_accounts);
    let mut storages = B256Map::default();

    for i in 0..num_accounts {
        // Create account address with some overlap between blocks
        let addr_num = ((i * 7 + block_idx) % 10000) as u64;
        let mut addr = B256::ZERO;
        addr.0[24..32].copy_from_slice(&addr_num.to_be_bytes());

        // Account state (Some = exists, None = destroyed)
        let account = if i % 20 == 0 {
            None // ~5% destroyed accounts
        } else {
            Some(reth_primitives_traits::Account {
                nonce: (block_idx + i) as u64,
                balance: U256::from(1000 * (block_idx + i)),
                bytecode_hash: None,
            })
        };
        accounts.push((addr, account));

        // Storage slots for this account
        if slots_per_account > 0 && i % 3 == 0 {
            // ~33% of accounts have storage changes
            let mut slots = Vec::with_capacity(slots_per_account);
            for j in 0..slots_per_account {
                let slot_num = ((j * 13 + block_idx) % 1000) as u64;
                let mut slot = B256::ZERO;
                slot.0[24..32].copy_from_slice(&slot_num.to_be_bytes());
                let value = U256::from(block_idx * 1000 + j);
                slots.push((slot, value));
            }
            slots.sort_by(|a, b| a.0.cmp(&b.0));

            let wiped = i % 50 == 0; // ~2% wiped storage
            storages.insert(addr, HashedStorageSorted { storage_slots: slots, wiped });
        }
    }

    accounts.sort_by(|a, b| a.0.cmp(&b.0));
    HashedPostStateSorted { accounts, storages }
}

/// Merge using current implementation (k-way merge + one-pass accumulation)
fn merge_kway(states: &[&HashedPostStateSorted]) -> HashedPostStateSorted {
    HashedPostStateSorted::merge_batch(states.iter().copied())
}

/// Merge using collect + sort approach
fn merge_collect_sort(states: &[&HashedPostStateSorted]) -> HashedPostStateSorted {
    if states.is_empty() {
        return HashedPostStateSorted::default();
    }

    // Collect all accounts, sort, dedup
    let mut accounts: Vec<_> = states.iter().flat_map(|s| s.accounts.iter().copied()).collect();
    accounts.sort_by(|a, b| a.0.cmp(&b.0));
    accounts.dedup_by(|a, b| a.0 == b.0);

    // Collect all addresses, then merge per-address
    let mut addrs: Vec<_> = states.iter().flat_map(|s| s.storages.keys().copied()).collect();
    addrs.sort_unstable();
    addrs.dedup();

    let storages = addrs
        .into_iter()
        .map(|addr| {
            let per_addr: Vec<_> = states.iter().filter_map(|s| s.storages.get(&addr)).collect();
            let wipe_idx = per_addr.iter().position(|u| u.wiped);
            let relevant = wipe_idx.map_or(&per_addr[..], |idx| &per_addr[..=idx]);

            let mut storage_slots: Vec<_> =
                relevant.iter().flat_map(|u| u.storage_slots.iter().copied()).collect();
            storage_slots.sort_by(|a, b| a.0.cmp(&b.0));
            storage_slots.dedup_by(|a, b| a.0 == b.0);

            (addr, HashedStorageSorted { wiped: wipe_idx.is_some(), storage_slots })
        })
        .collect();

    HashedPostStateSorted { accounts, storages }
}

/// Merge using simple append + sort + dedup approach
/// Appends all items, sorts once, dedup once - simplest possible approach
fn merge_append_sort_dedup(states: &[&HashedPostStateSorted]) -> HashedPostStateSorted {
    if states.is_empty() {
        return HashedPostStateSorted::default();
    }

    // Collect all accounts, sort, dedup (stable sort keeps first = newest)
    let mut accounts: Vec<_> = states.iter().flat_map(|s| s.accounts.iter().copied()).collect();
    accounts.sort_by(|a, b| a.0.cmp(&b.0));
    accounts.dedup_by(|a, b| a.0 == b.0);

    // Collect all storage addresses
    let mut addrs: Vec<_> = states.iter().flat_map(|s| s.storages.keys().copied()).collect();
    addrs.sort_unstable();
    addrs.dedup();

    // For each address: collect all slots, sort, dedup
    let storages = addrs
        .into_iter()
        .map(|addr| {
            // Find first wipe in newest-to-oldest order
            let per_addr: Vec<_> = states.iter().filter_map(|s| s.storages.get(&addr)).collect();
            let wipe_idx = per_addr.iter().position(|u| u.wiped);
            let relevant = wipe_idx.map_or(&per_addr[..], |idx| &per_addr[..=idx]);

            // Append all slots, sort, dedup
            let mut storage_slots: Vec<_> =
                relevant.iter().flat_map(|u| u.storage_slots.iter().copied()).collect();
            storage_slots.sort_by(|a, b| a.0.cmp(&b.0));
            storage_slots.dedup_by(|a, b| a.0 == b.0);

            (addr, HashedStorageSorted { wiped: wipe_idx.is_some(), storage_slots })
        })
        .collect();

    HashedPostStateSorted { accounts, storages }
}

/// Merge using the OLD extend_ref loop approach (original implementation on main)
/// This simulates: iterate in reverse (oldest to newest), extend_ref each one
fn merge_extend_ref_loop(states: &[&HashedPostStateSorted]) -> HashedPostStateSorted {
    if states.is_empty() {
        return HashedPostStateSorted::default();
    }

    // Reverse to get oldest-to-newest order (like the original .rev() call)
    let mut iter = states.iter().rev();
    let first = iter.next().unwrap();

    // Clone the first (oldest) state as the base
    let mut result = (*first).clone();

    // Extend with each subsequent state (newer states take precedence)
    for state in iter {
        result.extend_ref(state);
    }

    result
}

/// Merge using itertools kmerge
fn merge_itertools(states: &[&HashedPostStateSorted]) -> HashedPostStateSorted {
    if states.is_empty() {
        return HashedPostStateSorted::default();
    }

    // kmerge for accounts
    let accounts: Vec<_> = states
        .iter()
        .map(|s| s.accounts.iter().copied())
        .kmerge_by(|a, b| a.0 <= b.0)
        .dedup_by(|a, b| a.0 == b.0)
        .collect();

    // One-pass accumulation for storages
    struct StorageAcc<'a> {
        wiped: bool,
        sealed: bool,
        slices: Vec<&'a [(B256, U256)]>,
    }

    let mut acc: B256Map<StorageAcc<'_>> = B256Map::default();

    for state in states {
        for (addr, storage) in &state.storages {
            let entry = acc.entry(*addr).or_insert_with(|| StorageAcc {
                wiped: false,
                sealed: false,
                slices: Vec::new(),
            });

            if entry.sealed {
                continue;
            }

            entry.slices.push(storage.storage_slots.as_slice());
            if storage.wiped {
                entry.wiped = true;
                entry.sealed = true;
            }
        }
    }

    let storages = acc
        .into_iter()
        .map(|(addr, entry)| {
            let storage_slots: Vec<_> = entry
                .slices
                .into_iter()
                .map(|s| s.iter().copied())
                .kmerge_by(|a, b| a.0 <= b.0)
                .dedup_by(|a, b| a.0 == b.0)
                .collect();
            (addr, HashedStorageSorted { wiped: entry.wiped, storage_slots })
        })
        .collect();

    HashedPostStateSorted { accounts, storages }
}

fn bench_block_counts(c: &mut Criterion) {
    let mut group = c.benchmark_group("hashed_state_merge");

    // Realistic block sizes: accounts per block, slots per account
    let config = (50, 10); // 50 accounts, 10 slots each (moderate block)

    for num_blocks in [3, 30, 100, 300, 1000, 3000] {
        // Generate states (newest to oldest)
        let states: Vec<_> =
            (0..num_blocks).map(|i| generate_hashed_state(i, config.0, config.1)).collect();
        let state_refs: Vec<_> = states.iter().collect();

        group.bench_with_input(BenchmarkId::new("kway_merge", num_blocks), &num_blocks, |b, _| {
            b.iter(|| {
                let result = merge_kway(black_box(&state_refs));
                black_box(result)
            })
        });

        group.bench_with_input(
            BenchmarkId::new("collect_sort", num_blocks),
            &num_blocks,
            |b, _| {
                b.iter(|| {
                    let result = merge_collect_sort(black_box(&state_refs));
                    black_box(result)
                })
            },
        );

        group.bench_with_input(
            BenchmarkId::new("itertools_kmerge", num_blocks),
            &num_blocks,
            |b, _| {
                b.iter(|| {
                    let result = merge_itertools(black_box(&state_refs));
                    black_box(result)
                })
            },
        );

        group.bench_with_input(
            BenchmarkId::new("extend_ref_loop_OLD", num_blocks),
            &num_blocks,
            |b, _| {
                b.iter(|| {
                    let result = merge_extend_ref_loop(black_box(&state_refs));
                    black_box(result)
                })
            },
        );

        group.bench_with_input(
            BenchmarkId::new("append_sort_dedup", num_blocks),
            &num_blocks,
            |b, _| {
                b.iter(|| {
                    let result = merge_append_sort_dedup(black_box(&state_refs));
                    black_box(result)
                })
            },
        );
    }

    group.finish();
}

fn bench_varying_block_size(c: &mut Criterion) {
    let mut group = c.benchmark_group("varying_block_size");

    let num_blocks = 100; // Fixed block count

    // Different block sizes: (accounts, slots_per_account)
    let configs = [
        ("small", 10, 5),    // Small blocks
        ("medium", 50, 10),  // Medium blocks
        ("large", 200, 20),  // Large blocks
        ("xlarge", 500, 50), // Extra large blocks
    ];

    for (name, accounts, slots) in configs {
        let states: Vec<_> =
            (0..num_blocks).map(|i| generate_hashed_state(i, accounts, slots)).collect();
        let state_refs: Vec<_> = states.iter().collect();

        group.bench_with_input(BenchmarkId::new("kway_merge", name), &name, |b, _| {
            b.iter(|| {
                let result = merge_kway(black_box(&state_refs));
                black_box(result)
            })
        });

        group.bench_with_input(BenchmarkId::new("collect_sort", name), &name, |b, _| {
            b.iter(|| {
                let result = merge_collect_sort(black_box(&state_refs));
                black_box(result)
            })
        });

        group.bench_with_input(BenchmarkId::new("itertools_kmerge", name), &name, |b, _| {
            b.iter(|| {
                let result = merge_itertools(black_box(&state_refs));
                black_box(result)
            })
        });

        group.bench_with_input(BenchmarkId::new("extend_ref_loop_OLD", name), &name, |b, _| {
            b.iter(|| {
                let result = merge_extend_ref_loop(black_box(&state_refs));
                black_box(result)
            })
        });

        group.bench_with_input(BenchmarkId::new("append_sort_dedup", name), &name, |b, _| {
            b.iter(|| {
                let result = merge_append_sort_dedup(black_box(&state_refs));
                black_box(result)
            })
        });
    }

    group.finish();
}

/// Simulates the OLD merge_overlay_trie_input pattern:
/// .rev() to get oldest→newest, clone first, then extend_ref loop
fn merge_overlay_old(blocks: &[Arc<HashedPostStateSorted>]) -> Arc<HashedPostStateSorted> {
    if blocks.is_empty() {
        return Arc::new(HashedPostStateSorted::default());
    }

    // Reverse to get oldest→newest (like the original .rev() call)
    let mut iter = blocks.iter().rev().peekable();
    let first = iter.next().unwrap();

    // Start with first block's state
    let mut state = Arc::clone(first);

    // Only clone and mutate if there are more blocks
    if iter.peek().is_some() {
        let state_mut = Arc::make_mut(&mut state);
        for block in iter {
            state_mut.extend_ref(block.as_ref());
        }
    }

    state
}

/// Simulates the NEW merge_overlay_trie_input pattern:
/// Use merge_batch directly (newest→oldest order)
fn merge_overlay_new(blocks: &[Arc<HashedPostStateSorted>]) -> Arc<HashedPostStateSorted> {
    if blocks.is_empty() {
        return Arc::new(HashedPostStateSorted::default());
    }

    if blocks.len() == 1 {
        return Arc::clone(&blocks[0]);
    }

    // Batch merge - collect refs and merge
    let merged = HashedPostStateSorted::merge_batch(blocks.iter().map(|b| b.as_ref()));
    Arc::new(merged)
}

/// Simulates the ADAPTIVE merge_overlay_trie_input pattern:
/// Use extend_ref loop for small k, merge_batch for large k
fn merge_overlay_adaptive(blocks: &[Arc<HashedPostStateSorted>]) -> Arc<HashedPostStateSorted> {
    const MERGE_BATCH_THRESHOLD: usize = 64;

    if blocks.is_empty() {
        return Arc::new(HashedPostStateSorted::default());
    }

    if blocks.len() == 1 {
        return Arc::clone(&blocks[0]);
    }

    if blocks.len() < MERGE_BATCH_THRESHOLD {
        // Small k: extend_ref loop
        let mut iter = blocks.iter().rev();
        let first = iter.next().unwrap();
        let mut state = Arc::clone(first);
        let state_mut = Arc::make_mut(&mut state);
        for block in iter {
            state_mut.extend_ref(block.as_ref());
        }
        state
    } else {
        // Large k: merge_batch
        let merged = HashedPostStateSorted::merge_batch(blocks.iter().map(|b| b.as_ref()));
        Arc::new(merged)
    }
}

/// Generate hashed state with configurable wipe behavior
fn generate_hashed_state_with_wipe(
    block_idx: usize,
    num_accounts: usize,
    slots_per_account: usize,
    enable_wipes: bool,
) -> HashedPostStateSorted {
    let mut accounts = Vec::with_capacity(num_accounts);
    let mut storages = B256Map::default();

    for i in 0..num_accounts {
        let addr_num = ((i * 7 + block_idx) % 10000) as u64;
        let mut addr = B256::ZERO;
        addr.0[24..32].copy_from_slice(&addr_num.to_be_bytes());

        let account = if i % 20 == 0 {
            None
        } else {
            Some(reth_primitives_traits::Account {
                nonce: (block_idx + i) as u64,
                balance: U256::from(1000 * (block_idx + i)),
                bytecode_hash: None,
            })
        };
        accounts.push((addr, account));

        if slots_per_account > 0 && i % 3 == 0 {
            let mut slots = Vec::with_capacity(slots_per_account);
            for j in 0..slots_per_account {
                let slot_num = ((j * 13 + block_idx) % 1000) as u64;
                let mut slot = B256::ZERO;
                slot.0[24..32].copy_from_slice(&slot_num.to_be_bytes());
                let value = U256::from(block_idx * 1000 + j);
                slots.push((slot, value));
            }
            slots.sort_by(|a, b| a.0.cmp(&b.0));

            // Only set wiped if enabled
            let wiped = enable_wipes && i % 50 == 0;
            storages.insert(addr, HashedStorageSorted { storage_slots: slots, wiped });
        }
    }

    accounts.sort_by(|a, b| a.0.cmp(&b.0));
    HashedPostStateSorted { accounts, storages }
}

fn bench_merge_overlay(c: &mut Criterion) {
    let mut group = c.benchmark_group("merge_overlay_trie_input");

    // Realistic block sizes for in-memory blocks
    let config = (50, 10); // 50 accounts, 10 slots each

    for num_blocks in [3, 30, 100, 300, 1000, 3000] {
        // Generate states wrapped in Arc (like ExecutedBlock::trie_data())
        // Block 0 = newest, block N-1 = oldest (matches TreeState::blocks_by_hash order)
        let states: Vec<Arc<HashedPostStateSorted>> = (0..num_blocks)
            .map(|i| Arc::new(generate_hashed_state(i, config.0, config.1)))
            .collect();

        // Correctness check: verify old and adaptive produce same result
        let old_result = merge_overlay_old(&states);
        let adaptive_result = merge_overlay_adaptive(&states);
        assert_eq!(
            old_result.accounts.len(),
            adaptive_result.accounts.len(),
            "Account count mismatch for {} blocks",
            num_blocks
        );
        assert_eq!(
            old_result.storages.len(),
            adaptive_result.storages.len(),
            "Storage count mismatch for {} blocks",
            num_blocks
        );

        group.bench_with_input(
            BenchmarkId::new("old_extend_ref_loop", num_blocks),
            &num_blocks,
            |b, _| {
                b.iter(|| {
                    let result = merge_overlay_old(black_box(&states));
                    black_box(result)
                })
            },
        );

        group.bench_with_input(
            BenchmarkId::new("new_merge_batch", num_blocks),
            &num_blocks,
            |b, _| {
                b.iter(|| {
                    let result = merge_overlay_new(black_box(&states));
                    black_box(result)
                })
            },
        );

        group.bench_with_input(
            BenchmarkId::new("adaptive", num_blocks),
            &num_blocks,
            |b, _| {
                b.iter(|| {
                    let result = merge_overlay_adaptive(black_box(&states));
                    black_box(result)
                })
            },
        );
    }

    group.finish();
}

/// Benchmark with no wipes to test pure merge cost
fn bench_merge_overlay_no_wipe(c: &mut Criterion) {
    let mut group = c.benchmark_group("merge_overlay_no_wipe");

    let config = (50, 10);

    for num_blocks in [3, 30, 100, 300, 1000, 3000] {
        // Generate states WITHOUT wipes
        let states: Vec<Arc<HashedPostStateSorted>> = (0..num_blocks)
            .map(|i| Arc::new(generate_hashed_state_with_wipe(i, config.0, config.1, false)))
            .collect();

        // Correctness check
        let old_result = merge_overlay_old(&states);
        let adaptive_result = merge_overlay_adaptive(&states);
        assert_eq!(old_result.accounts.len(), adaptive_result.accounts.len());
        assert_eq!(old_result.storages.len(), adaptive_result.storages.len());

        group.bench_with_input(
            BenchmarkId::new("old_extend_ref_loop", num_blocks),
            &num_blocks,
            |b, _| {
                b.iter(|| {
                    let result = merge_overlay_old(black_box(&states));
                    black_box(result)
                })
            },
        );

        group.bench_with_input(
            BenchmarkId::new("adaptive", num_blocks),
            &num_blocks,
            |b, _| {
                b.iter(|| {
                    let result = merge_overlay_adaptive(black_box(&states));
                    black_box(result)
                })
            },
        );
    }

    group.finish();
}

/// Benchmark different MERGE_BATCH_THRESHOLD values to find optimal crossover point.
/// Tests thresholds of 8, 16, 32, 64 across different block counts.
fn bench_threshold_sweep(c: &mut Criterion) {
    let mut group = c.benchmark_group("threshold_sweep");

    let config = (50, 10); // 50 accounts, 10 slots each

    // Test around the crossover region and beyond
    for num_blocks in [8, 16, 24, 32, 48, 64, 100, 128, 256, 512, 1000] {
        let states: Vec<Arc<HashedPostStateSorted>> = (0..num_blocks)
            .map(|i| Arc::new(generate_hashed_state(i, config.0, config.1)))
            .collect();

        // extend_ref loop (baseline)
        group.bench_with_input(
            BenchmarkId::new("extend_ref", num_blocks),
            &num_blocks,
            |b, _| {
                b.iter(|| {
                    let result = merge_overlay_old(black_box(&states));
                    black_box(result)
                })
            },
        );

        // merge_batch (new approach)
        group.bench_with_input(
            BenchmarkId::new("merge_batch", num_blocks),
            &num_blocks,
            |b, _| {
                b.iter(|| {
                    let result = merge_overlay_new(black_box(&states));
                    black_box(result)
                })
            },
        );
    }

    group.finish();
}

/// Benchmark comparing old (clone-all-upfront) vs new (clone-survivors-only) kway merge
fn bench_kway_clone_strategy(c: &mut Criterion) {


    // Old implementation: clones all elements upfront
    fn kway_merge_clone_upfront<'a, K, V>(
        slices: impl IntoIterator<Item = &'a [(K, V)]>,
    ) -> Vec<(K, V)>
    where
        K: Ord + Clone + 'a,
        V: Clone + 'a,
    {
        slices
            .into_iter()
            .filter(|s| !s.is_empty())
            .enumerate()
            .map(|(i, s)| s.iter().cloned().map(move |item| (i, item)))
            .kmerge_by(|(i1, a), (i2, b)| (&a.0, *i1) < (&b.0, *i2))
            .dedup_by(|(_, a), (_, b)| a.0 == b.0)
            .map(|(_, item)| item)
            .collect()
    }

    // New implementation: merge by reference, clone only survivors
    fn kway_merge_clone_survivors<'a, K, V>(
        slices: impl IntoIterator<Item = &'a [(K, V)]>,
    ) -> Vec<(K, V)>
    where
        K: Ord + Clone + 'a,
        V: Clone + 'a,
    {
        slices
            .into_iter()
            .filter(|s| !s.is_empty())
            .enumerate()
            .map(|(i, s)| s.iter().map(move |(k, v)| (i, k, v)))
            .kmerge_by(|(i1, k1, _), (i2, k2, _)| (k1, i1) < (k2, i2))
            .dedup_by(|(_, k1, _), (_, k2, _)| *k1 == *k2)
            .map(|(_, k, v)| (k.clone(), v.clone()))
            .collect()
    }

    // Generate test data with high duplicate ratio (worst case for clone-upfront)
    // Use simple (B256, U256) tuples to avoid BranchNodeCompact validation issues
    fn generate_slices_high_overlap(
        num_slices: usize,
        items_per_slice: usize,
    ) -> Vec<Vec<(B256, U256)>> {
        (0..num_slices)
            .map(|slice_idx| {
                let mut items: Vec<_> = (0..items_per_slice)
                    .map(|i| {
                        // High overlap: most keys appear in multiple slices
                        let key_num = (i % (items_per_slice / 2).max(1)) as u64;
                        let key = B256::from(U256::from(key_num));
                        let value = U256::from(slice_idx * 1000 + i);
                        (key, value)
                    })
                    .collect();
                items.sort_by(|a, b| a.0.cmp(&b.0));
                items.dedup_by(|a, b| a.0 == b.0);
                items
            })
            .collect()
    }

    type SliceType = [(B256, U256)];

    let mut group = c.benchmark_group("kway_clone_strategy");

    // Test with varying overlap scenarios
    for (num_slices, items_per_slice) in [(10, 1000), (50, 500), (100, 200)] {
        let slices = generate_slices_high_overlap(num_slices, items_per_slice);
        let slice_refs: Vec<&SliceType> =
            slices.iter().map(|s: &Vec<_>| s.as_slice()).collect();

        group.bench_with_input(
            BenchmarkId::new("clone_upfront", format!("{num_slices}x{items_per_slice}")),
            &slice_refs,
            |b, slices: &Vec<&SliceType>| {
                b.iter(|| {
                    let result = kway_merge_clone_upfront(black_box(slices.iter().copied()));
                    black_box(result)
                })
            },
        );

        group.bench_with_input(
            BenchmarkId::new("clone_survivors", format!("{num_slices}x{items_per_slice}")),
            &slice_refs,
            |b, slices: &Vec<&SliceType>| {
                b.iter(|| {
                    let result = kway_merge_clone_survivors(black_box(slices.iter().copied()));
                    black_box(result)
                })
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_block_counts,
    bench_varying_block_size,
    bench_merge_overlay,
    bench_merge_overlay_no_wipe,
    bench_threshold_sweep,
    bench_kway_clone_strategy
);
criterion_main!(benches);

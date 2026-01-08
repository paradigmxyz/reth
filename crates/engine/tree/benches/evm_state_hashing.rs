//! Benchmarks comparing different strategies for hashing EVM state to HashedPostState.
//!
//! This benchmark tests the hypothesis that:
//! - Parallel hashing per-update has overhead that hurts performance for small updates
//! - Deferred batching (accumulate all updates, hash once at block end) is more efficient
//!
//! The benchmark simulates production conditions where:
//! - Most transactions touch only 1-5 accounts
//! - A block has many transactions (50-200+)
//! - State updates arrive rapidly during block execution

#![allow(missing_docs)]

use alloy_primitives::{keccak256, Address, B256};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use proptest::test_runner::TestRunner;
use rand::Rng;
use rayon::prelude::*;
use reth_trie::{HashedPostState, HashedStorage};
use revm_primitives::{HashMap, U256};
use revm_state::{Account as RevmAccount, AccountInfo, AccountStatus, EvmState, EvmStorageSlot};

/// Parameters for simulating realistic block execution
#[derive(Debug, Clone)]
struct BlockSimParams {
    /// Number of transactions in the block
    num_transactions: usize,
    /// Min accounts touched per transaction
    min_accounts_per_tx: usize,
    /// Max accounts touched per transaction
    max_accounts_per_tx: usize,
    /// Min storage slots changed per account
    min_storage_per_account: usize,
    /// Max storage slots changed per account
    max_storage_per_account: usize,
}

impl BlockSimParams {
    /// Typical mainnet block with moderate activity
    fn typical_block() -> Self {
        Self {
            num_transactions: 150,
            min_accounts_per_tx: 1,
            max_accounts_per_tx: 5,
            min_storage_per_account: 0,
            max_storage_per_account: 3,
        }
    }

    /// Heavy block with lots of state changes (e.g., popular NFT mint)
    fn heavy_block() -> Self {
        Self {
            num_transactions: 200,
            min_accounts_per_tx: 2,
            max_accounts_per_tx: 10,
            min_storage_per_account: 1,
            max_storage_per_account: 8,
        }
    }

    /// Light block with simple transfers
    fn light_block() -> Self {
        Self {
            num_transactions: 100,
            min_accounts_per_tx: 1,
            max_accounts_per_tx: 2,
            min_storage_per_account: 0,
            max_storage_per_account: 1,
        }
    }
}

/// Generate a single transaction's state update
fn generate_tx_state_update<R: Rng>(
    rng: &mut R,
    min_accounts: usize,
    max_accounts: usize,
    min_storage: usize,
    max_storage: usize,
) -> EvmState {
    let num_accounts = rng.random_range(min_accounts..=max_accounts);
    let mut state = EvmState::default();

    for _ in 0..num_accounts {
        let address = Address::random_with(rng);
        let num_slots = rng.random_range(min_storage..=max_storage);

        let storage: HashMap<U256, EvmStorageSlot> = (0..num_slots)
            .map(|_| {
                (
                    U256::from(rng.random::<u64>()),
                    EvmStorageSlot::new_changed(U256::ZERO, U256::from(rng.random::<u64>()), 0),
                )
            })
            .collect();

        let account = RevmAccount {
            info: AccountInfo {
                balance: U256::from(rng.random::<u64>()),
                nonce: rng.random::<u64>(),
                code_hash: Default::default(),
                code: None,
            },
            storage,
            status: AccountStatus::Touched,
            transaction_id: 0,
        };

        state.insert(address, account);
    }

    state
}

/// Generate all state updates for a block (one per transaction)
fn generate_block_state_updates(params: &BlockSimParams) -> Vec<EvmState> {
    let mut runner = TestRunner::deterministic();
    let mut rng = runner.rng().clone();
    (0..params.num_transactions)
        .map(|_| {
            generate_tx_state_update(
                &mut rng,
                params.min_accounts_per_tx,
                params.max_accounts_per_tx,
                params.min_storage_per_account,
                params.max_storage_per_account,
            )
        })
        .collect()
}

// ============================================================================
// STRATEGY 1: Sequential hashing per update (baseline)
// ============================================================================

fn hash_sequential(update: EvmState) -> HashedPostState {
    let mut hashed_state = HashedPostState::with_capacity(update.len());

    for (address, account) in update {
        if account.is_touched() {
            let hashed_address = keccak256(address);
            let destroyed = account.is_selfdestructed();
            let info = if destroyed { None } else { Some(account.info.into()) };
            hashed_state.accounts.insert(hashed_address, info);

            let mut changed_storage_iter = account
                .storage
                .into_iter()
                .filter(|(_slot, value)| value.is_changed())
                .map(|(slot, value)| (keccak256(B256::from(slot)), value.present_value))
                .peekable();

            if destroyed {
                hashed_state.storages.insert(hashed_address, HashedStorage::new(true));
            } else if changed_storage_iter.peek().is_some() {
                hashed_state
                    .storages
                    .insert(hashed_address, HashedStorage::from_iter(false, changed_storage_iter));
            }
        }
    }

    hashed_state
}

/// Process each update sequentially as it arrives (old behavior)
fn process_block_sequential_per_update(updates: Vec<EvmState>) -> Vec<HashedPostState> {
    updates.into_iter().map(hash_sequential).collect()
}

// ============================================================================
// STRATEGY 2: Parallel hashing per update (what caused regression)
// ============================================================================

fn hash_parallel(update: EvmState) -> HashedPostState {
    update
        .into_par_iter()
        .filter_map(|(address, account)| {
            if !account.is_touched() {
                return None;
            }

            let hashed_address = keccak256(address);
            let destroyed = account.is_selfdestructed();
            let info = if destroyed { None } else { Some(account.info.into()) };

            let hashed_storage = if destroyed {
                Some(HashedStorage::new(true))
            } else {
                let storage: Vec<_> = account
                    .storage
                    .into_iter()
                    .filter(|(_slot, value)| value.is_changed())
                    .map(|(slot, value)| (keccak256(B256::from(slot)), value.present_value))
                    .collect();

                if storage.is_empty() {
                    None
                } else {
                    Some(HashedStorage::from_iter(false, storage))
                }
            };

            Some((hashed_address, info, hashed_storage))
        })
        .collect()
}

/// Process each update with parallel hashing as it arrives (regression approach)
fn process_block_parallel_per_update(updates: Vec<EvmState>) -> Vec<HashedPostState> {
    updates.into_iter().map(hash_parallel).collect()
}

// ============================================================================
// STRATEGY 3: Deferred batching - accumulate all, hash once at end (new approach)
// ============================================================================

/// Merge all state updates into one, then hash in parallel
fn process_block_deferred_batched(updates: Vec<EvmState>) -> HashedPostState {
    // Merge all updates into one EvmState
    let mut merged = EvmState::default();
    for update in updates {
        merged.extend(update);
    }

    // Hash everything in parallel once
    hash_parallel(merged)
}

// ============================================================================
// BENCHMARKS
// ============================================================================

fn bench_hashing_strategies(c: &mut Criterion) {
    let mut group = c.benchmark_group("evm_state_hashing");

    let scenarios = [
        ("light_block", BlockSimParams::light_block()),
        ("typical_block", BlockSimParams::typical_block()),
        ("heavy_block", BlockSimParams::heavy_block()),
    ];

    for (name, params) in scenarios {
        // Generate test data
        let updates = generate_block_state_updates(&params);
        let total_accounts: usize = updates.iter().map(|u| u.len()).sum();

        group.throughput(Throughput::Elements(total_accounts as u64));

        // Benchmark sequential per-update
        group.bench_with_input(
            BenchmarkId::new("sequential_per_update", name),
            &updates,
            |b, updates| {
                b.iter(|| {
                    let updates_clone = updates.clone();
                    black_box(process_block_sequential_per_update(updates_clone))
                })
            },
        );

        // Benchmark parallel per-update (the regression approach)
        group.bench_with_input(
            BenchmarkId::new("parallel_per_update", name),
            &updates,
            |b, updates| {
                b.iter(|| {
                    let updates_clone = updates.clone();
                    black_box(process_block_parallel_per_update(updates_clone))
                })
            },
        );

        // Benchmark deferred batched (new approach)
        group.bench_with_input(
            BenchmarkId::new("deferred_batched", name),
            &updates,
            |b, updates| {
                b.iter(|| {
                    let updates_clone = updates.clone();
                    black_box(process_block_deferred_batched(updates_clone))
                })
            },
        );
    }

    group.finish();
}

/// Benchmark to understand the overhead of rayon for small collections
fn bench_rayon_overhead(c: &mut Criterion) {
    let mut group = c.benchmark_group("rayon_overhead");

    // Test with varying sizes to find the crossover point
    for num_accounts in [1, 2, 5, 10, 20, 50, 100, 200] {
        let mut runner = TestRunner::deterministic();
        let mut rng = runner.rng().clone();
        let update = generate_tx_state_update(&mut rng, num_accounts, num_accounts, 2, 2);

        group.throughput(Throughput::Elements(num_accounts as u64));

        group.bench_with_input(
            BenchmarkId::new("sequential", num_accounts),
            &update,
            |b, update| {
                b.iter(|| {
                    let update_clone = update.clone();
                    black_box(hash_sequential(update_clone))
                })
            },
        );

        group.bench_with_input(BenchmarkId::new("parallel", num_accounts), &update, |b, update| {
            b.iter(|| {
                let update_clone = update.clone();
                black_box(hash_parallel(update_clone))
            })
        });
    }

    group.finish();
}

criterion_group!(benches, bench_hashing_strategies, bench_rayon_overhead);
criterion_main!(benches);

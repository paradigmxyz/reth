#![allow(missing_docs)]

use alloy_consensus::transaction::TxHashRef;
use alloy_primitives::{TxHash, B256};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use reth_chain_state::{test_utils::TestBlockBuilder, BlockState, CanonicalInMemoryState};
use reth_ethereum_primitives::EthPrimitives;
use reth_primitives_traits::BlockBody;
use std::{collections::BTreeMap, sync::Arc};

criterion_group!(benches, bench_transaction_by_hash);
criterion_main!(benches);

/// Sets up an in-memory state with a given number of blocks and transactions per block.
fn setup_state_with_blocks(
    num_blocks: usize,
    txs_per_block: usize,
) -> (CanonicalInMemoryState<EthPrimitives>, Vec<TxHash>) {
    let mut builder = TestBlockBuilder::<EthPrimitives>::default();

    let mut blocks = alloy_primitives::map::HashMap::default();
    let mut numbers = BTreeMap::new();
    let mut tx_hashes = Vec::new();
    let mut parent_hash = B256::default();

    for block_num in 0..num_blocks {
        let executed =
            builder.get_executed_block_with_tx_count(block_num as u64, parent_hash, txs_per_block);
        let hash = executed.recovered_block().hash();
        parent_hash = hash;

        // Collect transaction hashes from this block
        for tx in executed.recovered_block().body().transactions() {
            tx_hashes.push(*tx.tx_hash());
        }

        let block_state = Arc::new(BlockState::new(executed));
        numbers.insert(block_num as u64, hash);
        blocks.insert(hash, block_state);
    }

    let state = CanonicalInMemoryState::new(blocks, numbers, None, None, None);
    (state, tx_hashes)
}

/// Simulates the old O(n) lookup behavior by iterating through all blocks.
fn transaction_by_hash_linear(
    state: &CanonicalInMemoryState<EthPrimitives>,
    hash: TxHash,
) -> Option<reth_ethereum_primitives::TransactionSigned> {
    for block_state in state.canonical_chain() {
        if let Some(tx) =
            block_state.block_ref().recovered_block().body().transaction_by_hash(&hash)
        {
            return Some(tx.clone())
        }
    }
    None
}

fn bench_transaction_by_hash(c: &mut Criterion) {
    let mut group = c.benchmark_group("transaction_by_hash");

    // Realistic scenario: 2-3 blocks with ~300 transactions each (mainnet-like)
    let scenarios = [
        ("2_blocks_300_txs", 2, 300),
        ("3_blocks_300_txs", 3, 300),
        ("2_blocks_500_txs", 2, 500),
        ("3_blocks_500_txs", 3, 500),
    ];

    for (name, num_blocks, txs_per_block) in scenarios {
        let (state, tx_hashes) = setup_state_with_blocks(num_blocks, txs_per_block);

        if tx_hashes.is_empty() {
            continue;
        }

        let total_txs = tx_hashes.len();

        // Benchmark lookup of first transaction (best case for linear - found immediately)
        let first_tx_hash = tx_hashes[0];
        group.bench_with_input(
            BenchmarkId::new(format!("{}/cached_first_tx", name), total_txs),
            &total_txs,
            |b, _| b.iter(|| black_box(state.transaction_by_hash(black_box(first_tx_hash)))),
        );

        group.bench_with_input(
            BenchmarkId::new(format!("{}/linear_first_tx", name), total_txs),
            &total_txs,
            |b, _| {
                b.iter(|| black_box(transaction_by_hash_linear(&state, black_box(first_tx_hash))))
            },
        );

        // Benchmark lookup of last transaction (worst case for linear - must scan everything)
        let last_tx_hash = *tx_hashes.last().unwrap();
        group.bench_with_input(
            BenchmarkId::new(format!("{}/cached_last_tx", name), total_txs),
            &total_txs,
            |b, _| b.iter(|| black_box(state.transaction_by_hash(black_box(last_tx_hash)))),
        );

        group.bench_with_input(
            BenchmarkId::new(format!("{}/linear_last_tx", name), total_txs),
            &total_txs,
            |b, _| {
                b.iter(|| black_box(transaction_by_hash_linear(&state, black_box(last_tx_hash))))
            },
        );

        // Benchmark lookup of middle transaction
        let mid_tx_hash = tx_hashes[total_txs / 2];
        group.bench_with_input(
            BenchmarkId::new(format!("{}/cached_mid_tx", name), total_txs),
            &total_txs,
            |b, _| b.iter(|| black_box(state.transaction_by_hash(black_box(mid_tx_hash)))),
        );

        group.bench_with_input(
            BenchmarkId::new(format!("{}/linear_mid_tx", name), total_txs),
            &total_txs,
            |b, _| b.iter(|| black_box(transaction_by_hash_linear(&state, black_box(mid_tx_hash)))),
        );

        // Benchmark lookup of non-existent transaction
        let non_existent = TxHash::random();
        group.bench_with_input(
            BenchmarkId::new(format!("{}/cached_miss", name), total_txs),
            &total_txs,
            |b, _| b.iter(|| black_box(state.transaction_by_hash(black_box(non_existent)))),
        );

        group.bench_with_input(
            BenchmarkId::new(format!("{}/linear_miss", name), total_txs),
            &total_txs,
            |b, _| {
                b.iter(|| black_box(transaction_by_hash_linear(&state, black_box(non_existent))))
            },
        );
    }

    group.finish();
}

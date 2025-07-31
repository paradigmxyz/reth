#![allow(missing_docs)]
use alloy_consensus::Transaction;
use alloy_primitives::{Address, B256, U256};
use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use proptest::{prelude::*, strategy::ValueTree, test_runner::TestRunner};
use rand::prelude::SliceRandom;
use reth_ethereum_primitives::{Block, BlockBody};
use reth_execution_types::ChangedAccount;
use reth_primitives_traits::{Header, SealedBlock};
use reth_transaction_pool::{
    test_utils::{MockTransaction, TestPoolBuilder},
    BlockInfo, CanonicalStateUpdate, PoolConfig, PoolTransaction, PoolUpdateKind, SubPoolLimit,
    TransactionOrigin, TransactionPool, TransactionPoolExt,
};
use std::{collections::HashMap, time::Duration};
/// Generates a set of transactions for multiple senders
fn generate_transactions(num_senders: usize, txs_per_sender: usize) -> Vec<MockTransaction> {
    let mut runner = TestRunner::deterministic();
    let mut txs = Vec::new();

    for sender_idx in 0..num_senders {
        // Create a unique sender address
        let sender_bytes = sender_idx.to_be_bytes();
        let addr_slice = [0u8; 12].into_iter().chain(sender_bytes.into_iter()).collect::<Vec<_>>();
        let sender = Address::from_slice(&addr_slice);

        // Generate transactions for this sender
        for nonce in 0..txs_per_sender {
            let mut tx = any::<MockTransaction>().new_tree(&mut runner).unwrap().current();
            tx.set_sender(sender);
            tx.set_nonce(nonce as u64);

            // Ensure it's not a legacy transaction
            if tx.is_legacy() || tx.is_eip2930() {
                tx = MockTransaction::eip1559();
                tx.set_priority_fee(any::<u128>().new_tree(&mut runner).unwrap().current());
                tx.set_max_fee(any::<u128>().new_tree(&mut runner).unwrap().current());
                tx.set_sender(sender);
                tx.set_nonce(nonce as u64);
            }

            txs.push(tx);
        }
    }

    txs
}

/// Fill the pool with transactions
async fn fill_pool(pool: &TestPoolBuilder, txs: Vec<MockTransaction>) -> HashMap<Address, u64> {
    let mut sender_nonces = HashMap::new();

    // Add transactions one by one
    for tx in txs {
        let sender = tx.sender();
        let nonce = tx.nonce();

        // Track the highest nonce for each sender
        sender_nonces.insert(sender, nonce.max(sender_nonces.get(&sender).copied().unwrap_or(0)));

        // Add transaction to the pool
        let _ = pool.add_transaction(TransactionOrigin::External, tx).await;
    }

    sender_nonces
}

fn canonical_state_change_bench(c: &mut Criterion) {
    let mut group = c.benchmark_group("Transaction Pool Canonical State Change");
    group.measurement_time(Duration::from_secs(10));
    let rt = tokio::runtime::Runtime::new().unwrap();
    // Test different pool sizes
    for num_senders in [500, 1000, 2000] {
        for txs_per_sender in [1, 5, 10] {
            let total_txs = num_senders * txs_per_sender;

            let group_id = format!(
                "txpool | canonical_state_change | senders: {num_senders} | txs_per_sender: {txs_per_sender} | total: {total_txs}",
            );

            // Create the update
            // Create a mock block - using default Ethereum block
            let header = Header::default();
            let body = BlockBody::default();
            let block = Block { header, body };
            let sealed_block = SealedBlock::seal_slow(block);

            let txs = generate_transactions(num_senders, txs_per_sender);
            let pool = TestPoolBuilder::default().with_config(PoolConfig {
                pending_limit: SubPoolLimit::max(),
                basefee_limit: SubPoolLimit::max(),
                queued_limit: SubPoolLimit::max(),
                blob_limit: SubPoolLimit::max(),
                max_account_slots: 50,
                ..Default::default()
            });
            struct Input<B: reth_primitives_traits::Block> {
                sealed_block: SealedBlock<B>,
                pool: TestPoolBuilder,
            }
            group.bench_with_input(group_id, &Input { sealed_block, pool }, |b, input| {
                b.iter_batched(
                    || {
                        // Setup phase - create pool and transactions
                        let sealed_block = &input.sealed_block;
                        let pool = &input.pool;
                        let senders = pool.unique_senders();
                        for sender in senders {
                            pool.remove_transactions_by_sender(sender);
                        }
                        // Set initial block info
                        pool.set_block_info(BlockInfo {
                            last_seen_block_number: 0,
                            last_seen_block_hash: B256::ZERO,
                            pending_basefee: 1_000_000_000,
                            pending_blob_fee: Some(1_000_000),
                            block_gas_limit: 30_000_000,
                        });
                        let sender_nonces = rt.block_on(fill_pool(pool, txs.clone()));
                        let mut changed_accounts: Vec<ChangedAccount> = sender_nonces
                            .into_iter()
                            .map(|(address, nonce)| ChangedAccount {
                                address,
                                nonce: nonce + 1, // Increment nonce as if transactions were mined
                                balance: U256::from(9_000_000_000_000_000u64), // Decrease balance
                            })
                            .collect();
                        changed_accounts.shuffle(&mut rand::rng());
                        let changed_accounts = changed_accounts.drain(..100).collect();
                        let update = CanonicalStateUpdate {
                            new_tip: sealed_block,
                            pending_block_base_fee: 1_000_000_000, // 1 gwei
                            pending_block_blob_fee: Some(1_000_000), // 0.001 gwei
                            changed_accounts,
                            mined_transactions: vec![], // No transactions mined in this benchmark
                            update_kind: PoolUpdateKind::Commit,
                        };

                        (pool, update)
                    },
                    |(pool, update)| {
                        // The actual operation being benchmarked
                        pool.on_canonical_state_change(update);
                    },
                    BatchSize::LargeInput,
                );
            });
        }
    }

    group.finish();
}

criterion_group! {
    name = canonical_state_change;
    config = Criterion::default();
    targets = canonical_state_change_bench
}
criterion_main!(canonical_state_change);

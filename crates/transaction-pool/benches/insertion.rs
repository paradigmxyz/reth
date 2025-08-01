#![allow(missing_docs)]
use alloy_primitives::Address;
use criterion::{criterion_group, criterion_main, Criterion};
use proptest::{prelude::*, strategy::ValueTree, test_runner::TestRunner};
use reth_transaction_pool::{
    batcher::{BatchTxProcessor, BatchTxRequest},
    test_utils::{testing_pool, MockTransaction},
    TransactionOrigin, TransactionPool,
};
use tokio::sync::oneshot;

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

/// Benchmark individual transaction insertion
fn txpool_insertion(c: &mut Criterion) {
    let mut group = c.benchmark_group("Txpool insertion");
    let scenarios = [(1000, 100), (5000, 500), (10000, 1000), (20000, 2000)];

    for (tx_count, sender_count) in scenarios {
        let group_id = format!("txs: {tx_count} | senders: {sender_count}");

        group.bench_function(group_id, |b| {
            b.iter_with_setup(
                || {
                    let rt = tokio::runtime::Runtime::new().unwrap();
                    let pool = testing_pool();
                    let txs = generate_transactions(tx_count, sender_count);
                    (rt, pool, txs)
                },
                |(rt, pool, txs)| {
                    rt.block_on(async {
                        for tx in &txs {
                            let _ =
                                pool.add_transaction(TransactionOrigin::Local, tx.clone()).await;
                        }
                    });
                },
            );
        });
    }

    group.finish();
}

/// Benchmark batch transaction insertion
fn txpool_batch_insertion(c: &mut Criterion) {
    let mut group = c.benchmark_group("Txpool batch insertion");
    let scenarios = [(1000, 100), (5000, 500), (10000, 1000), (20000, 2000)];

    for (tx_count, sender_count) in scenarios {
        let group_id = format!("txs: {tx_count} | senders: {sender_count}");

        group.bench_function(group_id, |b| {
            b.iter_with_setup(
                || {
                    let rt = tokio::runtime::Runtime::new().unwrap();
                    let pool = testing_pool();
                    let txs = generate_transactions(tx_count, sender_count);
                    let (processor, request_tx) = BatchTxProcessor::new(pool, tx_count);
                    let processor_handle = rt.spawn(processor);

                    let mut batch_requests = Vec::with_capacity(tx_count);
                    let mut response_futures = Vec::with_capacity(tx_count);
                    for tx in txs {
                        let (response_tx, response_rx) = oneshot::channel();
                        let request = BatchTxRequest::new(tx, response_tx);
                        batch_requests.push(request);
                        response_futures.push(response_rx);
                    }

                    (rt, request_tx, processor_handle, batch_requests, response_futures)
                },
                |(rt, request_tx, _processor_handle, batch_requests, response_futures)| {
                    rt.block_on(async {
                        // Send all transactions
                        for request in batch_requests {
                            request_tx.send(request).unwrap();
                        }

                        for response_rx in response_futures {
                            let _res = response_rx.await.unwrap();
                        }
                    });
                },
            );
        });
    }

    group.finish();
}

criterion_group! {
    name = insertion;
    config = Criterion::default();
    targets = txpool_insertion, txpool_batch_insertion
}
criterion_main!(insertion);

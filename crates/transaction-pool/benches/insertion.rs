#![allow(missing_docs)]
use alloy_primitives::{Address, U256};
use criterion::{criterion_group, criterion_main, Criterion};
use reth_transaction_pool::{
    batcher::{BatchTxProcessor, BatchTxRequest},
    test_utils::{testing_pool, MockTransaction},
    TransactionOrigin, TransactionPool,
};
use tokio::sync::oneshot;

/// Generate simple transfer transactions
fn generate_transactions(count: usize, sender_count: usize) -> Vec<MockTransaction> {
    let mut transactions = Vec::with_capacity(count);

    let senders: Vec<Address> = (0..sender_count)
        .map(|i| {
            let mut bytes = [0u8; 20];
            bytes[16..20].copy_from_slice(&(i as u32).to_be_bytes());
            Address::from(bytes)
        })
        .collect();

    for i in 0..count {
        let sender_idx = i % sender_count;
        let nonce = (i / sender_count) as u64;
        let sender = senders[sender_idx];

        let mut tx = MockTransaction::legacy();

        tx.set_sender(sender);
        tx.set_nonce(nonce);
        tx.set_gas_price(100);
        tx.set_gas_limit(21_000);
        tx.set_value(U256::ONE);

        transactions.push(tx);
    }

    transactions
}

/// Benchmark individual transaction insertion
fn txpool_insertion(c: &mut Criterion) {
    let mut group = c.benchmark_group("Txpool insertion");
    let scenarios = [(1000, 100), (5000, 500), (10000, 1000), (20000, 2000)];

    for (tx_count, sender_count) in scenarios {
        let group_id = format!("txs: {} | senders: {}", tx_count, sender_count);

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
        let group_id = format!("txs: {} | senders: {}", tx_count, sender_count);

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

#![allow(missing_docs)]
use alloy_primitives::U256;
use criterion::*;
use pprof::criterion::{Output, PProfProfiler};
use rand::thread_rng;
use reth_network::{
    test_utils::Testnet,
    transactions::{
        TransactionFetcherConfig, TransactionPropagationMode::Max, TransactionsManagerConfig,
    },
};
use reth_provider::test_utils::{ExtendedAccount, MockEthProvider};
use reth_transaction_pool::{test_utils::TransactionGenerator, PoolTransaction, TransactionPool};
use tokio::runtime::Runtime as TokioRuntime;

criterion_group!(
    name = tx_fetch_benches;
    config = Criterion::default().with_profiler(PProfProfiler::new(100, Output::Flamegraph(None)));
    targets = tx_fetch_bench
);

pub fn tx_fetch_bench(c: &mut Criterion) {
    let rt = TokioRuntime::new().unwrap();

    let mut group = c.benchmark_group("Transaction Fetch");
    group.sample_size(10);

    group.bench_function("fetch_transactions", |b| {
        b.to_async(&rt).iter_with_setup(
            || {
                tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        let tx_manager_config = TransactionsManagerConfig {
                            propagation_mode: Max(0),
                            transaction_fetcher_config: TransactionFetcherConfig {
                                max_inflight_requests: 1,
                                ..Default::default()
                            },
                            ..Default::default()
                        };

                        let provider = MockEthProvider::default();
                        let num_peers = 10;
                        let net = Testnet::create_with(num_peers, provider.clone()).await;

                        // install request handlers
                        let net = net.with_eth_pool_config(tx_manager_config);
                        let handle = net.spawn();

                        // connect all the peers first
                        handle.connect_peers().await;

                        let listening_peer = &handle.peers()[num_peers - 1];
                        let listening_peer_tx_listener =
                            listening_peer.pool().unwrap().pending_transactions_listener();

                        let num_tx_per_peer = 10;

                        for i in 1..num_peers {
                            let peer = &handle.peers()[i];
                            let peer_pool = peer.pool().unwrap();

                            for _ in 0..num_tx_per_peer {
                                let mut gen = TransactionGenerator::new(thread_rng());
                                let tx = gen.gen_eip1559_pooled();
                                let sender = tx.sender();
                                provider.add_account(
                                    sender,
                                    ExtendedAccount::new(0, U256::from(100_000_000)),
                                );
                                peer_pool.add_external_transaction(tx.clone()).await.unwrap();
                            }
                        }

                        // Total expected transactions
                        let total_expected_tx = num_tx_per_peer * (num_peers - 1);

                        (listening_peer_tx_listener, total_expected_tx)
                    })
                })
            },
            |(mut listening_peer_tx_listener, total_expected_tx)| async move {
                let mut received_tx = 0;
                while listening_peer_tx_listener.recv().await.is_some() {
                    received_tx += 1;
                    if received_tx >= total_expected_tx {
                        break;
                    }
                }
            },
        )
    });

    group.finish();
}

criterion_main!(tx_fetch_benches);

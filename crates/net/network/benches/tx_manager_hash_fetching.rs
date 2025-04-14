#![allow(missing_docs)]

use alloy_primitives::{B256, U256};
use criterion::{measurement::WallTime, *};
use rand::SeedableRng;
use reth_eth_wire::EthVersion;
use reth_eth_wire_types::EthNetworkPrimitives;
use reth_network::{
    test_utils::{
        transactions::{buffer_hash_to_tx_fetcher, new_mock_session},
        Testnet,
    },
    transactions::{
        fetcher::TransactionFetcher, TransactionFetcherConfig, TransactionPropagationMode::Max,
        TransactionsManagerConfig,
    },
};
use reth_network_peers::PeerId;
use reth_provider::test_utils::{ExtendedAccount, MockEthProvider};
use reth_transaction_pool::{test_utils::TransactionGenerator, PoolTransaction, TransactionPool};
use std::collections::HashMap;
use tokio::runtime::Runtime as TokioRuntime;

criterion_group!(
    name = tx_fetch_benches;
    config = Criterion::default();
    targets = tx_fetch_bench, fetch_pending_hashes,
);

pub fn benchmark_fetch_pending_hashes(group: &mut BenchmarkGroup<'_, WallTime>, peers_num: usize) {
    let setup = || {
        let mut tx_fetcher = TransactionFetcher::<EthNetworkPrimitives>::default();
        let mut peers = HashMap::default();

        for _i in 0..peers_num {
            // NOTE: the worst case, each tx in the cache belongs to a differenct peer.
            let peer = PeerId::random();
            let hash = B256::random();

            let (mut peer_data, _) = new_mock_session(peer, EthVersion::Eth66);
            peer_data.seen_transactions_mut().insert(hash);
            peers.insert(peer, peer_data);

            buffer_hash_to_tx_fetcher(&mut tx_fetcher, hash, peer, 0, None);
        }

        (tx_fetcher, peers)
    };

    let group_id = format!("fetch pending hashes, peers num: {}", peers_num);

    group.bench_function(group_id, |b| {
        b.iter_with_setup(setup, |(mut tx_fetcher, peers)| {
            tx_fetcher.on_fetch_pending_hashes(&peers, |_| true);
        });
    });
}

pub fn fetch_pending_hashes(c: &mut Criterion) {
    let mut group = c.benchmark_group("Fetch Pending Hashes");

    for peers in [5, 10, 20, 100, 1000, 10000, 100000] {
        benchmark_fetch_pending_hashes(&mut group, peers);
    }

    group.finish();
}

pub fn tx_fetch_bench(c: &mut Criterion) {
    let rt = TokioRuntime::new().unwrap();

    let mut group = c.benchmark_group("Transaction Fetch");
    group.sample_size(30);

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
                                let mut gen =
                                    TransactionGenerator::new(rand::rngs::StdRng::seed_from_u64(0));

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

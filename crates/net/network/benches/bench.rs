#![allow(missing_docs)]
use criterion::*;
use futures::StreamExt;
use pprof::criterion::{Output, PProfProfiler};
use rand::thread_rng;
use reth_network::{test_utils::Testnet, NetworkEvents};
use reth_network_api::Peers;
use reth_primitives::U256;
use reth_provider::test_utils::{ExtendedAccount, MockEthProvider};
use reth_transaction_pool::{test_utils::TransactionGenerator, PoolTransaction};
use std::sync::Arc;
use tokio::{runtime::Runtime as TokioRuntime, sync::mpsc::unbounded_channel};

criterion_group!(
    name = broadcast_benches;
    config = Criterion::default().with_profiler(PProfProfiler::new(100, Output::Flamegraph(None)));
    targets = broadcast_ingress_bench
);

pub fn broadcast_ingress_bench(c: &mut Criterion) {
    let rt = TokioRuntime::new().unwrap();

    let mut group = c.benchmark_group("Broadcast Ingress");
    group.sample_size(10);
    group.bench_function("receive_broadcasts", move |b| {
        b.to_async(&rt).iter_with_setup(
            || {
                // `b.to_async(rt)` automatically enters the
                // runtime context and simply calling `block_on` here will cause the code to panic.
                tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        let provider = MockEthProvider::default();
                        let mut net = Testnet::create_with(2, provider.clone()).await;

                        let mut peer0 = net.remove_peer(0);
                        let (tx, transactions_rx) = unbounded_channel();
                        peer0.network_mut().set_transactions(tx);
                        let mut events0 = peer0.handle().event_listener();
                        let net = net.with_eth_pool();
                        let handle = net.spawn();
                        let peer1 = handle.peers()[0].network().clone();
                        let peer0_id = peer0.peer_id();
                        peer1.add_peer(peer0_id, peer0.local_addr());

                        // await connection
                        tokio::select! {
                            _ = events0.next() => {}
                            _ = &mut peer0 => {}
                        }

                        // prepare some transactions
                        let mut gen = TransactionGenerator::new(thread_rng());
                        let num_broadcasts = 10;
                        for _ in 0..num_broadcasts {
                            for _ in 0..2 {
                                let mut txs = Vec::new();
                                let tx = gen.gen_eip1559_pooled();
                                // ensure the sender has balance
                                provider.add_account(
                                    tx.sender(),
                                    ExtendedAccount::new(0, U256::from(100_000_000)),
                                );
                                txs.push(Arc::new(tx.transaction().clone().into_signed()));
                                peer1.send_transactions(peer0_id, txs);
                            }
                        }
                        (num_broadcasts, transactions_rx, peer0, handle)
                    })
                })
            },
            |(num_txs, mut transactions_rx, mut peer0, _handle)| async move {
                let mut count = 0;
                loop {
                    tokio::select! {
                        _ = transactions_rx.recv() => {
                            count += 1;
                            if count == num_txs {
                                break;
                            }
                        },
                        _ = &mut peer0 => {
                        }
                    }
                }
            },
        )
    });
}

criterion_main!(broadcast_benches);

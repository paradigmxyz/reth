//! Multi-node transaction propagation harness for benchmarking and profiling.
//!
//! Spawns a configurable number of in-process nodes, each running its own `NetworkManager` and
//! `TransactionsManager` on a dedicated tokio task, connects them in a full mesh and repeatedly
//! injects batches of transactions into the first node's pool. For every round it measures how
//! long it takes until each transaction has reached the pending pool of every other node.
//!
//! Run standalone:
//!
//! ```sh
//! cargo run --release -p reth-network --example tx_propagation_bench -- [NODES] [TXS_PER_ROUND] [ROUNDS]
//! ```
//!
//! Generate a CPU profile with samply:
//!
//! ```sh
//! cargo build --profile profiling -p reth-network --example tx_propagation_bench
//! samply record target/profiling/examples/tx_propagation_bench 8 500 50
//! ```
//!
//! Note: total transactions (`TXS_PER_ROUND * ROUNDS`) should stay below the default pool
//! capacity (10k pending transactions), otherwise pool eviction adds noise to the measurements.

use alloy_primitives::U256;
use reth_network::{
    test_utils::{NetworkEventStream, Testnet},
    Peers,
};
use reth_primitives_traits::SignedTransaction;
use reth_provider::test_utils::{ExtendedAccount, MockEthProvider};
use reth_transaction_pool::{
    test_utils::TransactionGenerator, EthPooledTransaction, PoolTransaction, TransactionPool,
};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    reth_tracing::init_test_tracing();

    let mut args = std::env::args().skip(1);
    let num_nodes: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(8);
    let txs_per_round: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(500);
    let rounds: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(10);
    // injecting in small batches resembles live gossip (txs trickling in), one big batch
    // stresses the bulk import/propagation path
    let batch_size: usize =
        args.next().and_then(|s| s.parse().ok()).unwrap_or(txs_per_round).max(1);

    println!(
        "tx propagation bench: {num_nodes} nodes (full mesh), {txs_per_round} txs/round, {rounds} rounds, injected in batches of {batch_size}"
    );

    let provider = MockEthProvider::default().with_genesis_block();
    let net = Testnet::create_with(num_nodes, provider.clone()).await;
    let mut net = net.with_eth_pool();

    // Spawn every peer on its own task so the nodes actually run in parallel, instead of all
    // being polled by a single task as `Testnet::spawn` does.
    let mut handles = Vec::with_capacity(num_nodes);
    while !net.peers().is_empty() {
        let peer = net.remove_peer(0);
        handles.push(peer.peer_handle());
        tokio::spawn(peer);
    }

    // connect all peers in a full mesh
    let streams: Vec<_> =
        handles.iter().map(|h| NetworkEventStream::new(h.event_listener())).collect();
    for (idx, handle) in handles.iter().enumerate() {
        for neighbour in &handles[idx + 1..] {
            handle.network().add_peer(*neighbour.peer_id(), neighbour.local_addr());
        }
    }
    let sessions_per_peer = num_nodes - 1;
    futures::future::join_all(
        streams
            .into_iter()
            .map(|mut s| async move { s.take_session_established(sessions_per_peer).await }),
    )
    .await;
    println!("all sessions established");

    let source_pool = handles[0].pool().unwrap();
    let mut round_durations = Vec::with_capacity(rounds);
    let mut all_latencies = Vec::new();

    for round in 0..rounds {
        // generate the batch with consecutive nonces per signer and fund each sender so pool
        // validation passes. fresh signers every round avoid nonce overlap with prior rounds.
        let mut tx_gen =
            TransactionGenerator::with_num_signers(rand::rng(), (txs_per_round / 4).max(10));
        let mut nonces = HashMap::new();
        let mut txs = Vec::with_capacity(txs_per_round);
        for _ in 0..txs_per_round {
            let builder = tx_gen.transaction();
            let nonce = nonces.entry(builder.signer).or_insert(0u64);
            let signed = builder.nonce(*nonce).into_eip1559();
            *nonce += 1;
            let tx = EthPooledTransaction::try_from_consensus(
                signed.try_into_recovered().expect("generated transaction is recoverable"),
            )
            .expect("generated transaction is a valid pooled transaction");
            provider.add_account(tx.sender(), ExtendedAccount::new(0, U256::from(1u128 << 100)));
            txs.push(tx);
        }

        // subscribe on all sink nodes before injecting
        let collectors: Vec<_> = handles[1..]
            .iter()
            .map(|handle| {
                let mut listener = handle.pool().unwrap().pending_transactions_listener();
                tokio::spawn(async move {
                    let mut arrivals = Vec::with_capacity(txs_per_round);
                    while arrivals.len() < txs_per_round {
                        match listener.recv().await {
                            Some(hash) => arrivals.push((hash, Instant::now())),
                            None => break,
                        }
                    }
                    arrivals
                })
            })
            .collect();

        // inject the transactions in batches, recording the insertion time per transaction
        let mut injected_at = HashMap::with_capacity(txs_per_round);
        let mut txs = txs.into_iter().peekable();
        while txs.peek().is_some() {
            let batch: Vec<_> = txs.by_ref().take(batch_size).collect();
            let now = Instant::now();
            for tx in &batch {
                injected_at.insert(*tx.hash(), now);
            }
            let outcomes = source_pool.add_external_transactions(batch).await;
            let ok = outcomes.iter().filter(|res| res.is_ok()).count();
            assert_eq!(ok, outcomes.len(), "all generated transactions should be valid");
        }

        let mut round_latencies = Vec::with_capacity(sessions_per_peer * txs_per_round);
        for collector in collectors {
            let arrivals = tokio::time::timeout(Duration::from_secs(60), collector)
                .await
                .expect("propagation timed out")
                .unwrap();
            assert_eq!(arrivals.len(), txs_per_round, "node did not receive all transactions");
            round_latencies.extend(
                arrivals.into_iter().map(|(hash, at)| at.duration_since(injected_at[&hash])),
            );
        }

        // time until the last transaction reached the last node
        let elapsed = round_latencies.iter().max().copied().unwrap_or_default();
        round_durations.push(elapsed);

        round_latencies.sort_unstable();
        println!(
            "round {:>3}/{rounds}: full coverage in {:>10.3?} | latency p50 {:>10.3?} p90 {:>10.3?} p99 {:>10.3?}",
            round + 1,
            elapsed,
            percentile(&round_latencies, 0.50),
            percentile(&round_latencies, 0.90),
            percentile(&round_latencies, 0.99),
        );
        all_latencies.extend(round_latencies);
    }

    all_latencies.sort_unstable();
    round_durations.sort_unstable();
    let total_tx_deliveries = all_latencies.len();
    println!("\nsummary ({rounds} rounds, {total_tx_deliveries} tx deliveries):");
    println!(
        "  coverage time: p50 {:>10.3?} max {:>10.3?}",
        percentile(&round_durations, 0.50),
        round_durations.last().copied().unwrap_or_default(),
    );
    println!(
        "  tx latency:    p50 {:>10.3?} p90 {:>10.3?} p99 {:>10.3?} max {:>10.3?}",
        percentile(&all_latencies, 0.50),
        percentile(&all_latencies, 0.90),
        percentile(&all_latencies, 0.99),
        all_latencies.last().copied().unwrap_or_default(),
    );
}

/// Returns the percentile value from the sorted slice.
fn percentile(sorted: &[Duration], pct: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let idx = ((sorted.len() as f64 * pct) as usize).min(sorted.len() - 1);
    sorted[idx]
}

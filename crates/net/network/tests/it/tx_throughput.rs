//! Ad-hoc two-peer transaction propagation throughput measurement.
//!
//! Not run in CI; execute manually with:
//! `cargo nextest run -p reth-network --run-ignored all -E 'test(tx_propagation_throughput)'
//! --no-capture`

use alloy_primitives::{Address, B256, U256};
use reth_network::test_utils::Testnet;
use reth_primitives_traits::SignedTransaction;
use reth_provider::test_utils::{ExtendedAccount, MockEthProvider};
use reth_transaction_pool::{
    test_utils::TransactionBuilder, EthPooledTransaction, PoolTransaction, TransactionPool,
};
use std::time::{Duration, Instant};

const NUM_SIGNERS: usize = 800;
const TXS_PER_SIGNER: usize = 10;
const CHUNK: usize = 500;

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "throughput measurement, run manually"]
async fn tx_propagation_throughput() {
    reth_tracing::init_test_tracing();

    let provider = MockEthProvider::default().with_genesis_block();
    let net = Testnet::create_with(2, provider.clone()).await;
    let net = net.with_eth_pool();
    let handle = net.spawn();
    handle.connect_peers().await;

    let peer0_pool = handle.peers()[0].pool().unwrap().clone();
    let mut peer1_listener = handle.peers()[1].pool().unwrap().pending_transactions_listener();

    // pre-generate all transactions: unique signers with sequential nonces so everything lands in
    // the pending subpool
    let signers: Vec<B256> = (0..NUM_SIGNERS).map(|_| B256::random()).collect();
    let mut txs = Vec::with_capacity(NUM_SIGNERS * TXS_PER_SIGNER);
    for nonce in 0..TXS_PER_SIGNER {
        for key in &signers {
            let tx = TransactionBuilder::default()
                .signer(*key)
                .nonce(nonce as u64)
                .gas_limit(21_000)
                .max_fee_per_gas(20)
                .max_priority_fee_per_gas(10)
                .to(Address::repeat_byte(0x42))
                .value(1)
                .into_eip1559();
            let pooled =
                EthPooledTransaction::try_from_consensus(tx.try_into_recovered().unwrap()).unwrap();
            txs.push(pooled);
        }
    }
    let total = txs.len();

    // fund all senders
    for tx in txs.iter().take(NUM_SIGNERS) {
        provider.add_account(
            tx.sender(),
            ExtendedAccount::new(0, U256::from(1_000_000_000_000_000_000u64)),
        );
    }

    // Closed-loop: cap the number of in-flight (inserted but not yet received) transactions so
    // the session's broadcast backpressure (4096 queued items) never drops messages, which would
    // otherwise stall the run since a 2-peer network has no other source to recover them from.
    const MAX_IN_FLIGHT: usize = 1024;

    let start = Instant::now();

    let mut inserted = 0usize;
    let mut received = 0usize;
    for chunk in txs.chunks(CHUNK) {
        let results = peer0_pool.add_external_transactions(chunk.to_vec()).await;
        for res in &results {
            if let Err(err) = res {
                panic!("insert failed after {inserted} txs: {err}");
            }
        }
        inserted += results.len();

        while inserted - received > MAX_IN_FLIGHT {
            match tokio::time::timeout(Duration::from_secs(20), peer1_listener.recv()).await {
                Ok(Some(_)) => received += 1,
                other => panic!("propagation stalled at {received}/{total}: {other:?}"),
            }
        }
    }
    while received < total {
        match tokio::time::timeout(Duration::from_secs(20), peer1_listener.recv()).await {
            Ok(Some(_)) => received += 1,
            other => panic!("propagation stalled at {received}/{total}: {other:?}"),
        }
    }
    let elapsed = start.elapsed();

    assert_eq!(inserted, total, "all generated txs must be inserted");

    println!(
        "propagated {total} txs in {elapsed:?} -> {:.0} tx/s",
        total as f64 / elapsed.as_secs_f64()
    );
}

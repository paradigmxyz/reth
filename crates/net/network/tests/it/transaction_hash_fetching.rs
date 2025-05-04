use alloy_primitives::U256;
use rand::thread_rng;
use reth_network::{
    test_utils::Testnet,
    transactions::{TransactionPropagationMode::Max, TransactionsManagerConfig},
};
use reth_provider::test_utils::{ExtendedAccount, MockEthProvider};
use reth_tracing::init_test_tracing;
use reth_transaction_pool::{test_utils::TransactionGenerator, PoolTransaction, TransactionPool};
use tokio::time::Duration;

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn transaction_hash_fetching() {
    init_test_tracing();

    let mut config = TransactionsManagerConfig { propagation_mode: Max(0), ..Default::default() };
    config.transaction_fetcher_config.max_inflight_requests = 1;

    let provider = MockEthProvider::default();
    let num_peers = 10;
    let net = Testnet::create_with(num_peers, provider.clone()).await;

    // install request handlers
    let net = net.with_eth_pool_config(config);
    let handle = net.spawn();

    // connect all the peers first
    handle.connect_peers().await;

    let listening_peer = &handle.peers()[num_peers - 1];
    let mut listening_peer_tx_listener =
        listening_peer.pool().unwrap().pending_transactions_listener();

    let num_tx_per_peer = 10;

    // Generate transactions for peers
    for i in 1..num_peers {
        let peer = &handle.peers()[i];
        let peer_pool = peer.pool().unwrap();

        for _ in 0..num_tx_per_peer {
            let mut gen = TransactionGenerator::new(thread_rng());
            let tx = gen.gen_eip1559_pooled();
            let sender = tx.sender();
            provider.add_account(sender, ExtendedAccount::new(0, U256::from(100_000_000)));
            peer_pool.add_external_transaction(tx).await.unwrap();
        }
    }

    // Total expected transactions
    let total_expected_tx = num_tx_per_peer * (num_peers - 1);
    let mut received_tx = 0;

    loop {
        tokio::select! {
            Some(_) = listening_peer_tx_listener.recv() => {
                received_tx += 1;
                if received_tx >= total_expected_tx {
                    break;
                }
            }
            _ = tokio::time::sleep(Duration::from_secs(10)) => {
                panic!("Timed out waiting for transactions. Received {received_tx}/{total_expected_tx}");
            }
        }
    }
}

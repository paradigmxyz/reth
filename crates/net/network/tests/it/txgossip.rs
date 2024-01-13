//! Testing gossiping of transactions.

use rand::thread_rng;
use reth_network::test_utils::Testnet;
use reth_primitives::U256;
use reth_provider::test_utils::{ExtendedAccount, MockEthProvider};
use reth_transaction_pool::{test_utils::TransactionGenerator, PoolTransaction, TransactionPool};
#[tokio::test(flavor = "multi_thread")]
async fn test_tx_gossip() {
    reth_tracing::init_test_tracing();

    let provider = MockEthProvider::default();
    let net = Testnet::create_with(2, provider.clone()).await;

    // install request handlers
    let net = net.with_eth_pool();
    let handle = net.spawn();
    // connect all the peers
    handle.connect_peers().await;

    let peer0 = &handle.peers()[0];
    let peer1 = &handle.peers()[1];

    let peer0_pool = peer0.pool().unwrap();
    let mut peer0_tx_listener = peer0.pool().unwrap().pending_transactions_listener();
    let mut peer1_tx_listener = peer1.pool().unwrap().pending_transactions_listener();

    let mut gen = TransactionGenerator::new(thread_rng());
    let tx = gen.gen_eip1559_pooled();

    // ensure the sender has balance
    let sender = tx.sender();
    provider.add_account(sender, ExtendedAccount::new(0, U256::from(100_000_000)));

    // insert pending tx in peer0's pool
    let hash = peer0_pool.add_external_transaction(tx).await.unwrap();

    let inserted = peer0_tx_listener.recv().await.unwrap();
    assert_eq!(inserted, hash);

    // ensure tx is gossiped to peer1
    let received = peer1_tx_listener.recv().await.unwrap();
    assert_eq!(received, hash);
}

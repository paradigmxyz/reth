use alloy_primitives::{Signature, B256};
use reth_eth_wire::{GetPooledTransactions, PooledTransactions};
use reth_ethereum_primitives::TransactionSigned;
use reth_network::{test_utils::Testnet, PeerRequest};
use reth_primitives_traits::SignedTransaction;
use reth_provider::test_utils::MockEthProvider;
use reth_transaction_pool::{
    test_utils::{testing_pool, MockTransaction},
    TransactionPool,
};

// peer0: `GetPooledTransactions` requester
// peer1: `GetPooledTransactions` responder
#[tokio::test(flavor = "multi_thread")]
async fn test_large_tx_req() {
    reth_tracing::init_test_tracing();

    // create 2000 fake txs
    let txs: Vec<MockTransaction> = (0..2000)
        .map(|_| {
            // replace rng txhash with real txhash
            let mut tx = MockTransaction::eip1559();

            let ts =
                TransactionSigned::new_unhashed(tx.clone().into(), Signature::test_signature());
            tx.set_hash(ts.recalculate_hash());
            tx
        })
        .collect();
    let txs_hashes: Vec<B256> = txs.iter().map(|tx| *tx.get_hash()).collect();

    // setup testnet
    let mut net = Testnet::create_with(2, MockEthProvider::default()).await.with_request_handlers();

    // insert generated txs into responding peer's pool
    let pool1 = testing_pool();
    pool1.add_external_transactions(txs).await;

    // install transactions managers
    net.peers_mut()[0].install_transactions_manager(testing_pool());
    net.peers_mut()[1].install_transactions_manager(pool1);

    let net = net.spawn();
    net.connect_peers().await;
    let [peer0, peer1] = net.peers_array();

    // make `GetPooledTransactions` request
    let request = GetPooledTransactions(txs_hashes.clone());
    let PooledTransactions(txs) = peer0
        .request(*peer1.peer_id(), |response| PeerRequest::GetPooledTransactions {
            request,
            response,
        })
        .await
        .unwrap();

    // check all txs have been received
    for tx in txs {
        assert!(txs_hashes.contains(tx.hash()));
    }
}

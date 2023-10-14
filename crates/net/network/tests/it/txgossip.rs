//! Testing gossiping of transactions.

use reth_network::test_utils::Testnet;

use reth_provider::test_utils::MockEthProvider;

#[tokio::test(flavor = "multi_thread")]
async fn test_tx_gossip() {
    reth_tracing::init_test_tracing();

    let mut net = Testnet::create_with(2, MockEthProvider::default()).await;

    // install request handlers
    net.for_each_mut(|peer| peer.install_test_pool());
    let handle = net.spawn();
    // connect all the peers
    handle.connect_peers().await;
}

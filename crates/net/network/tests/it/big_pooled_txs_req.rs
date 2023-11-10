use reth_eth_wire::{GetPooledTransactions, PooledTransactions};
use reth_interfaces::sync::{NetworkSyncUpdater, SyncState};
use reth_network::{
    test_utils::{NetworkEventStream, Testnet},
    NetworkEvents, PeerRequest,
};
use reth_network_api::{NetworkInfo, Peers};
use reth_primitives::{Signature, TransactionSigned, B256};
use reth_provider::test_utils::MockEthProvider;
use reth_transaction_pool::{
    test_utils::{testing_pool, MockTransaction},
    TransactionPool,
};
use tokio::sync::oneshot;

// peer0: `GetPooledTransactions` requestor
// peer1: `GetPooledTransactions` responder
#[tokio::test(flavor = "multi_thread")]
async fn test_large_tx_req() {
    reth_tracing::init_test_tracing();

    // create 2000 fake txs
    let txs: Vec<MockTransaction> = (0..2000)
        .map(|_| {
            // replace rng txhash with real txhash
            let mut tx = MockTransaction::eip1559();

            let ts = TransactionSigned {
                hash: Default::default(),
                signature: Signature::default(),
                transaction: tx.clone().into(),
            };
            tx.set_hash(ts.recalculate_hash());
            tx
        })
        .collect();
    let txs_hashes: Vec<B256> = txs.iter().map(|tx| tx.get_hash()).collect();

    // setup testnet
    let mut net = Testnet::create_with(2, MockEthProvider::default()).await;

    // install request handlers
    net.for_each_mut(|peer| peer.install_request_handler());

    // insert generated txs into responding peer's pool
    let pool1 = testing_pool();
    pool1.add_external_transactions(txs).await.unwrap();

    // install transactions managers
    net.peers_mut()[0].install_transactions_manager(testing_pool());
    net.peers_mut()[1].install_transactions_manager(pool1);

    // connect peers together and check for connection existance
    let handle0 = net.peers()[0].handle();
    let handle1 = net.peers()[1].handle();
    let mut events0 = NetworkEventStream::new(handle0.event_listener());

    let _handle = net.spawn();

    handle0.add_peer(*handle1.peer_id(), handle1.local_addr());
    let connected = events0.next_session_established().await.unwrap();
    assert_eq!(connected, *handle1.peer_id());

    // stop syncing
    handle0.update_sync_state(SyncState::Idle);
    handle1.update_sync_state(SyncState::Idle);
    assert!(!handle0.is_syncing() && !handle1.is_syncing());

    // make `GetPooledTransactions` request
    let (send, receive) = oneshot::channel();
    handle0.send_request(
        *handle1.peer_id(),
        PeerRequest::GetPooledTransactions {
            request: GetPooledTransactions(txs_hashes.clone()),
            response: send,
        },
    );

    // check all txs have been received
    match receive.await.unwrap() {
        Ok(PooledTransactions(txs)) => {
            txs.into_iter().for_each(|tx| assert!(txs_hashes.contains(tx.hash())));
        }
        Err(e) => {
            panic!("error: {:?}", e);
        }
    }
}

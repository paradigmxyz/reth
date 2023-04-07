use reth_eth_wire::{GetPooledTransactions, PooledTransactions, Transactions};
use reth_interfaces::{p2p::error::RequestResult, sync::SyncStateUpdater};
use reth_network::{
    test_utils::Testnet, transactions::NetworkTransactionEvent, NetworkConfigBuilder, NetworkEvent,
    NetworkManager,
};
use reth_network_api::{NetworkInfo, Peers};
use reth_rlp::Decodable;

use reth_primitives::TransactionSigned;
use reth_provider::test_utils::NoopProvider;
use reth_transaction_pool::{
    test_utils::{testing_pool, MockTransaction},
    TransactionPool,
};
use secp256k1::SecretKey;
use tokio::sync::oneshot;
use tokio_stream::StreamExt;

#[tokio::test(flavor = "multi_thread")]
async fn test_handle_incoming_transactions() {
    reth_tracing::init_test_tracing();
    let net = Testnet::create(3).await;

    let mut handles = net.handles();
    let handle0 = handles.next().unwrap();
    let handle1 = handles.next().unwrap();

    drop(handles);
    let handle = net.spawn();

    let listener0 = handle0.event_listener();

    handle0.add_peer(*handle1.peer_id(), handle1.local_addr());
    let secret_key = SecretKey::new(&mut rand::thread_rng());

    let client = NoopProvider::default();
    let pool = testing_pool();
    let config = NetworkConfigBuilder::new(secret_key).build(client);
    let (network_handle, network, mut transactions, _) = NetworkManager::new(config)
        .await
        .unwrap()
        .into_builder()
        .transactions(pool.clone())
        .split_with_handle();
    tokio::task::spawn(network);

    network_handle.update_sync_state(reth_interfaces::sync::SyncState::Idle);

    assert!(!network_handle.is_syncing());

    // wait for all initiator connections
    let mut established = listener0.take(2);
    while let Some(ev) = established.next().await {
        match ev {
            NetworkEvent::SessionEstablished {
                peer_id,
                capabilities,
                messages,
                status,
                version,
            } => {
                // to insert a new peer in transactions peerset
                transactions.on_network_event(NetworkEvent::SessionEstablished {
                    peer_id,
                    capabilities,
                    messages,
                    status,
                    version,
                })
            }
            NetworkEvent::PeerAdded(_peer_id) => continue,
            _ => {
                panic!("unexpected event")
            }
        }
    }
    // random tx: <https://etherscan.io/getRawTx?tx=0x9448608d36e721ef403c53b00546068a6474d6cbab6816c3926de449898e7bce>
    let input = hex::decode("02f871018302a90f808504890aef60826b6c94ddf4c5025d1a5742cf12f74eec246d4432c295e487e09c3bbcc12b2b80c080a0f21a4eacd0bf8fea9c5105c543be5a1d8c796516875710fafafdf16d16d8ee23a001280915021bb446d1973501a67f93d2b38894a514b976e7b46dc2fe54598d76").unwrap();
    let signed_tx = TransactionSigned::decode(&mut &input[..]).unwrap();
    transactions.on_network_tx_event(NetworkTransactionEvent::IncomingTransactions {
        peer_id: handle1.peer_id().clone(),
        msg: Transactions(vec![signed_tx.clone()]),
    });
    assert_eq!(
        *handle1.peer_id(),
        transactions.transactions_by_peers.get(&signed_tx.hash()).unwrap()[0]
    );
    handle.terminate().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_on_get_pooled_transactions_network() {
    reth_tracing::init_test_tracing();
    let net = Testnet::create(2).await;

    let mut handles = net.handles();
    let handle0 = handles.next().unwrap();
    let handle1 = handles.next().unwrap();

    drop(handles);
    let handle = net.spawn();

    let listener0 = handle0.event_listener();

    handle0.add_peer(*handle1.peer_id(), handle1.local_addr());
    let secret_key = SecretKey::new(&mut rand::thread_rng());

    let client = NoopProvider::default();
    let pool = testing_pool();
    let config = NetworkConfigBuilder::new(secret_key).build(client);
    let (network_handle, network, mut transactions, _) = NetworkManager::new(config)
        .await
        .unwrap()
        .into_builder()
        .transactions(pool.clone())
        .split_with_handle();
    tokio::task::spawn(network);

    network_handle.update_sync_state(reth_interfaces::sync::SyncState::Idle);

    assert!(!network_handle.is_syncing());

    // wait for all initiator connections
    let mut established = listener0.take(2);
    while let Some(ev) = established.next().await {
        match ev {
            NetworkEvent::SessionEstablished {
                peer_id,
                capabilities,
                messages,
                status,
                version,
            } => transactions.on_network_event(NetworkEvent::SessionEstablished {
                peer_id,
                capabilities,
                messages,
                status,
                version,
            }),
            NetworkEvent::PeerAdded(_peer_id) => continue,
            _ => {
                panic!("unexpected event")
            }
        }
    }
    handle.terminate().await;

    let tx = MockTransaction::eip1559();
    let _ = transactions
        .pool
        .add_transaction(reth_transaction_pool::TransactionOrigin::External, tx.clone())
        .await;

    let request = GetPooledTransactions(vec![tx.get_hash()]);

    let (send, receive) = oneshot::channel::<RequestResult<PooledTransactions>>();

    transactions.on_network_tx_event(NetworkTransactionEvent::GetPooledTransactions {
        peer_id: handle1.peer_id().clone(),
        request,
        response: send,
    });

    match receive.await.unwrap() {
        Ok(PooledTransactions(transactions)) => {
            assert_eq!(transactions.len(), 1);
        }
        Err(e) => {
            panic!("error: {:?}", e);
        }
    }
}

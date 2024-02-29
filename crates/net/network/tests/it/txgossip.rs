//! Testing gossiping of transactions.

use futures::StreamExt;
use rand::thread_rng;
use reth_network::{test_utils::Testnet, NetworkEvent, NetworkEvents};
use reth_network_api::PeersInfo;
use reth_primitives::{TransactionSigned, TxLegacy, U256};
use reth_provider::test_utils::{ExtendedAccount, MockEthProvider};
use reth_transaction_pool::{test_utils::TransactionGenerator, PoolTransaction, TransactionPool};
use std::sync::Arc;

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

#[tokio::test(flavor = "multi_thread")]
async fn test_4844_tx_gossip_penalization() {
    reth_tracing::init_test_tracing();
    let provider = MockEthProvider::default();
    let net = Testnet::create_with(2, provider.clone()).await;

    // install request handlers
    let net = net.with_eth_pool();

    let handle = net.spawn();

    let peer0 = &handle.peers()[0];
    let peer1 = &handle.peers()[1];

    // connect all the peers
    handle.connect_peers().await;

    let mut peer1_tx_listener = peer1.pool().unwrap().pending_transactions_listener();

    let mut gen = TransactionGenerator::new(thread_rng());

    // peer 0 will be penalised for sending txs[0] over gossip
    let txs = vec![gen.gen_eip4844_pooled(), gen.gen_eip1559_pooled()];

    txs.iter().for_each(|tx| {
        let sender = tx.sender();
        provider.add_account(sender, ExtendedAccount::new(0, U256::from(100_000_000)));
    });

    let signed_txs: Vec<Arc<TransactionSigned>> =
        txs.iter().map(|tx| Arc::new(tx.transaction().clone().into_signed())).collect();

    let network_handle = peer0.network();

    let peer0_reputation_before =
        peer1.peer_handle().peer_by_id(*peer0.peer_id()).await.unwrap().reputation();

    // sends txs directly to peer1
    network_handle.send_transactions(*peer1.peer_id(), signed_txs);

    let received = peer1_tx_listener.recv().await.unwrap();

    let peer0_reputation_after =
        peer1.peer_handle().peer_by_id(*peer0.peer_id()).await.unwrap().reputation();
    assert_ne!(peer0_reputation_before, peer0_reputation_after);
    assert_eq!(received, txs[1].transaction().hash);

    // this will return an [`Empty`] error because blob txs are disallowed to be broadcasted
    assert!(peer1_tx_listener.try_recv().is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_sending_invalid_transactions() {
    reth_tracing::init_test_tracing();
    let provider = MockEthProvider::default();
    let net = Testnet::create_with(2, provider.clone()).await;
    // install request handlers
    let net = net.with_eth_pool();

    let handle = net.spawn();

    let peer0 = &handle.peers()[0];
    let peer1 = &handle.peers()[1];

    // connect all the peers
    handle.connect_peers().await;

    assert_eq!(peer0.network().num_connected_peers(), 1);
    let mut peer1_events = peer1.network().event_listener();
    let mut tx_listener = peer1.pool().unwrap().new_transactions_listener();

    for idx in 0..10 {
        // send invalid txs to peer1
        let tx = TxLegacy {
            chain_id: None,
            nonce: idx,
            gas_price: 0,
            gas_limit: 0,
            to: Default::default(),
            value: Default::default(),
            input: Default::default(),
        };
        let tx = TransactionSigned::from_transaction_and_signature(tx.into(), Default::default());
        peer0.network().send_transactions(*peer1.peer_id(), vec![Arc::new(tx)]);
    }

    // await disconnect for bad tx spam
    if let Some(ev) = peer1_events.next().await {
        match ev {
            NetworkEvent::SessionClosed { peer_id, .. } => {
                assert_eq!(peer_id, *peer0.peer_id());
            }
            NetworkEvent::SessionEstablished { .. } => {
                panic!("unexpected SessionEstablished event")
            }
            NetworkEvent::PeerAdded(_) => {
                panic!("unexpected PeerAdded event")
            }
            NetworkEvent::PeerRemoved(_) => {
                panic!("unexpected PeerRemoved event")
            }
        }
    }

    // ensure txs never made it to the pool
    assert!(tx_listener.try_recv().is_err());
}

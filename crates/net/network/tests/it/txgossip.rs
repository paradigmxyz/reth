//! Testing gossiping of transactions.
use crate::utils::{funded_transaction, poll_until};
use alloy_consensus::TxLegacy;
use alloy_primitives::{map::B256Set, Signature, TxHash, U256};
use reth_ethereum_primitives::TransactionSigned;
use reth_network::{
    test_utils::{PeerHandle, Testnet},
    transactions::config::{
        TransactionIngressPolicy, TransactionPropagationKind, TransactionsManagerConfig,
    },
    Peers,
};
use reth_network_api::{PeerId, PeerKind, PeersInfo};
use reth_provider::test_utils::{ExtendedAccount, MockEthProvider};
use reth_transaction_pool::{test_utils::TransactionGenerator, PoolTransaction, TransactionPool};
use std::{sync::Arc, time::Duration};
use tokio::join;

#[tokio::test(flavor = "multi_thread")]
async fn test_tx_gossip() {
    reth_tracing::init_test_tracing();

    let provider = MockEthProvider::default().with_genesis_block();
    let net = Testnet::create_with(2, provider.clone()).await.with_eth_pool().spawn();
    net.connect_peers().await;
    let [peer0, peer1] = net.peers_array();

    let mut peer0_tx_listener = peer0.pool().unwrap().pending_transactions_listener();
    let mut peer1_tx_listener = peer1.pool().unwrap().pending_transactions_listener();

    // insert pending tx in peer0's pool
    let tx = funded_transaction(&provider);
    let hash = peer0.pool().unwrap().add_external_transaction(tx).await.unwrap().hash;
    assert_eq!(peer0_tx_listener.recv().await.unwrap(), hash);

    // ensure tx is gossiped to peer1
    assert_eq!(peer1_tx_listener.recv().await.unwrap(), hash);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_tx_propagation_policy_trusted_only() {
    reth_tracing::init_test_tracing();

    let provider = MockEthProvider::default().with_genesis_block();
    let net = Testnet::create_with(2, provider.clone())
        .await
        .with_eth_pool_config_and_policy(Default::default(), TransactionPropagationKind::Trusted)
        .spawn();
    net.connect_peers().await;
    let [peer0, peer1] = net.peers_array();

    let mut peer0_tx_listener = peer0.pool().unwrap().pending_transactions_listener();
    let mut peer1_tx_listener = peer1.pool().unwrap().pending_transactions_listener();

    // insert the tx in peer0's pool
    let tx = funded_transaction(&provider);
    let outcome_0 = peer0.pool().unwrap().add_external_transaction(tx).await.unwrap();
    assert_eq!(peer0_tx_listener.recv().await.unwrap(), outcome_0.hash);

    // ensure tx is not gossiped to peer1
    peer1_tx_listener.try_recv().expect_err("Empty");

    let mut event_stream_0 = peer0.event_stream();
    let mut event_stream_1 = peer1.event_stream();

    // disconnect peer1 from peer0
    peer0.network().remove_peer(*peer1.peer_id(), PeerKind::Static);
    join!(event_stream_0.next_session_closed(), event_stream_1.next_session_closed());

    // re register peer1 as trusted
    peer0.add_trusted_peer(peer1);
    join!(event_stream_0.next_session_established(), event_stream_1.next_session_established());

    // insert pending tx in peer0's pool
    let tx = funded_transaction(&provider);
    let outcome_1 = peer0.pool().unwrap().add_external_transaction(tx).await.unwrap();
    assert_eq!(peer0_tx_listener.recv().await.unwrap(), outcome_1.hash);

    // ensure peer1 now receives the pending txs from peer0
    let mut buff = Vec::with_capacity(2);
    buff.push(peer1_tx_listener.recv().await.unwrap());
    buff.push(peer1_tx_listener.recv().await.unwrap());

    assert!(buff.contains(&outcome_1.hash));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_tx_ingress_policy_trusted_only() {
    reth_tracing::init_test_tracing();

    let provider = MockEthProvider::default().with_genesis_block();
    let tx_manager_config = TransactionsManagerConfig {
        ingress_policy: TransactionIngressPolicy::Trusted,
        ..Default::default()
    };
    let net = Testnet::create_with(2, provider.clone())
        .await
        .with_eth_pool_config(tx_manager_config)
        .spawn();
    net.connect_peers().await;
    let [peer0, peer1] = net.peers_array();

    let mut peer0_tx_listener = peer0.pool().unwrap().pending_transactions_listener();

    // insert the tx in peer1's pool
    let tx = funded_transaction(&provider);
    let outcome_0 = peer1.pool().unwrap().add_external_transaction(tx).await.unwrap();

    // ensure tx is not accepted by peer0
    peer0_tx_listener.try_recv().expect_err("Empty");

    let mut event_stream_0 = peer0.event_stream();
    let mut event_stream_1 = peer1.event_stream();

    // disconnect peer1 from peer0
    peer0.network().remove_peer(*peer1.peer_id(), PeerKind::Static);
    join!(event_stream_0.next_session_closed(), event_stream_1.next_session_closed());

    // re register peer1 as trusted
    peer0.add_trusted_peer(peer1);
    join!(event_stream_0.next_session_established(), event_stream_1.next_session_established());

    // insert pending tx in peer1's pool
    let tx = funded_transaction(&provider);
    let outcome_1 = peer1.pool().unwrap().add_external_transaction(tx).await.unwrap();

    // ensure peer0 now receives both pending txs from peer1 (the blocked one and the new one)
    let mut buff = Vec::with_capacity(2);
    buff.push(peer0_tx_listener.recv().await.unwrap());
    buff.push(peer0_tx_listener.recv().await.unwrap());

    assert!(buff.contains(&outcome_0.hash));
    assert!(buff.contains(&outcome_1.hash));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_tx_propagation_policy_trusted_only_on_connect() {
    reth_tracing::init_test_tracing();

    let provider = MockEthProvider::default().with_genesis_block();
    let net = Testnet::create_with(3, provider.clone())
        .await
        .with_eth_pool_config_and_policy(Default::default(), TransactionPropagationKind::Trusted)
        .spawn();
    let [peer0, untrusted, trusted] = net.peers_array();
    let pool0 = peer0.pool().unwrap();

    // insert the tx before connecting, so it can only be learned from the announcement of the
    // pending pool sent on session establishment
    let pending = pool0.add_external_transaction(funded_transaction(&provider)).await.unwrap().hash;

    connect_untrusted_and_trusted(peer0, untrusted, trusted).await;

    // ensure the trusted peer learns about the pending tx
    wait_for_announcement(trusted, peer0, pending).await;

    // announcements to a peer are delivered in order, so the untrusted peer receives any
    // announcement sent on session establishment before this one
    let marker = pool0.add_external_transaction(funded_transaction(&provider)).await.unwrap().hash;
    peer0.transactions().unwrap().propagate_hash_to(marker, *untrusted.peer_id());

    // ensure the untrusted peer never learned about the pending tx
    let announced = wait_for_announcement(untrusted, peer0, marker).await;
    assert!(!announced.contains(&pending));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_tx_propagation_policy_trusted_only_get_pooled_transactions() {
    reth_tracing::init_test_tracing();

    let provider = MockEthProvider::default().with_genesis_block();
    let net = Testnet::create_with(3, provider.clone())
        .await
        .with_eth_pool_config_and_policy(Default::default(), TransactionPropagationKind::Trusted)
        .spawn();
    let [peer0, untrusted, trusted] = net.peers_array();

    let pending = peer0
        .pool()
        .unwrap()
        .add_external_transaction(funded_transaction(&provider))
        .await
        .unwrap()
        .hash;

    connect_untrusted_and_trusted(peer0, untrusted, trusted).await;

    // ensure the untrusted peer can't request the pending tx
    let txs = untrusted
        .transactions()
        .unwrap()
        .get_pooled_transactions_from(*peer0.peer_id(), vec![pending])
        .await
        .unwrap()
        .unwrap();
    assert!(txs.is_empty());

    // ensure the trusted peer can
    let txs = trusted
        .transactions()
        .unwrap()
        .get_pooled_transactions_from(*peer0.peer_id(), vec![pending])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(txs.iter().map(|tx| *tx.hash()).collect::<Vec<_>>(), vec![pending]);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_4844_tx_gossip_penalization() {
    reth_tracing::init_test_tracing();

    let provider = MockEthProvider::default().with_genesis_block();
    let net = Testnet::create_with(2, provider.clone()).await.with_eth_pool().spawn();
    net.connect_peers().await;
    let [peer0, peer1] = net.peers_array();

    let mut peer1_tx_listener = peer1.pool().unwrap().pending_transactions_listener();

    let mut tx_gen = TransactionGenerator::new(rand::rng());

    // peer 0 will be penalized for sending txs[0] over gossip
    let txs = vec![tx_gen.gen_eip4844_pooled(), tx_gen.gen_eip1559_pooled()];

    for tx in &txs {
        provider.add_account(tx.sender(), ExtendedAccount::new(0, U256::from(100_000_000)));
    }

    let signed_txs: Vec<Arc<TransactionSigned>> =
        txs.iter().map(|tx| Arc::new(tx.transaction().clone().into_inner())).collect();

    let peer0_reputation_before =
        peer1.peer_handle().peer_by_id(*peer0.peer_id()).await.unwrap().reputation();

    // sends txs directly to peer1
    peer0.network().send_transactions(*peer1.peer_id(), signed_txs);

    let received = peer1_tx_listener.recv().await.unwrap();

    let peer0_reputation_after =
        peer1.peer_handle().peer_by_id(*peer0.peer_id()).await.unwrap().reputation();
    assert_ne!(peer0_reputation_before, peer0_reputation_after);
    assert_eq!(received, *txs[1].transaction().tx_hash());

    // this will return an [`Empty`] error because blob txs are disallowed to be broadcasted
    assert!(peer1_tx_listener.try_recv().is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_sending_invalid_transactions() {
    reth_tracing::init_test_tracing();

    let provider = MockEthProvider::default().with_genesis_block();
    let net = Testnet::create_with(2, provider).await.with_eth_pool().spawn();
    let [peer0, peer1] = net.peers_array();
    let mut peer1_events = peer1.event_stream();
    net.connect_peers().await;

    assert_eq!(peer0.network().num_connected_peers(), 1);
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
        let tx = TransactionSigned::new_unhashed(tx.into(), Signature::test_signature());
        peer0.network().send_transactions(*peer1.peer_id(), vec![Arc::new(tx)]);
    }

    // The listener also receives connection events, and PeerAdded can still be queued after
    // connect_peers returns. Wait specifically for the disconnect after bad transaction spam.
    let (peer_id, _) =
        tokio::time::timeout(Duration::from_secs(10), peer1_events.next_session_closed())
            .await
            .expect("peer did not disconnect after invalid transaction spam")
            .expect("network event stream ended before disconnect");
    assert_eq!(peer_id, *peer0.peer_id());

    // ensure txs never made it to the pool
    assert!(tx_listener.try_recv().is_err());
}

/// Connects `peer` to `untrusted` as a basic peer and to `trusted` as a trusted peer, and waits
/// until the transactions managers of all peers handled the new sessions.
async fn connect_untrusted_and_trusted<Pool>(
    peer: &PeerHandle<Pool>,
    untrusted: &PeerHandle<Pool>,
    trusted: &PeerHandle<Pool>,
) {
    peer.add_peer(untrusted);
    peer.add_trusted_peer(trusted);

    wait_for_active_peers(peer, &[*untrusted.peer_id(), *trusted.peer_id()]).await;
    wait_for_active_peers(untrusted, &[*peer.peer_id()]).await;
    wait_for_active_peers(trusted, &[*peer.peer_id()]).await;
}

/// Waits until the transactions manager of `peer` has active sessions with all `peers`.
async fn wait_for_active_peers<Pool>(peer: &PeerHandle<Pool>, peers: &[PeerId]) {
    let transactions = peer.transactions().unwrap();
    poll_until("sessions", async || {
        let active = transactions.get_active_peers().await.unwrap();
        peers.iter().all(|peer| active.contains(peer)).then_some(())
    })
    .await
}

/// Waits until `peer` learned from `announcer` that it has `hash`, and returns all hashes `peer`
/// knows `announcer` has.
async fn wait_for_announcement<Pool>(
    peer: &PeerHandle<Pool>,
    announcer: &PeerHandle<Pool>,
    hash: TxHash,
) -> B256Set {
    let transactions = peer.transactions().unwrap();
    poll_until("announcement", async || {
        let hashes = transactions.get_peer_transaction_hashes(*announcer.peer_id()).await.unwrap();
        hashes.contains(&hash).then_some(hashes)
    })
    .await
}

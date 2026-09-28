//! Connection tests

use alloy_primitives::map::HashSet;
use futures::StreamExt;
use reth_chainspec::SEPOLIA;
use reth_eth_wire::{DisconnectReason, HeadersDirection};
use reth_network::{
    config::rng_secret_key,
    test_utils::{PeerConfig, Testnet},
    BlockDownloaderProvider, NetworkEvent, PeersConfig,
};
use reth_network_api::{
    events::{PeerEvent, SessionInfo},
    PeerKind, Peers, PeersInfo,
};
use reth_network_p2p::{
    headers::client::{HeadersClient, HeadersRequest},
    sync::{NetworkSyncUpdater, SyncState},
};
use reth_network_peers::{NodeRecord, TrustedPeer};
use reth_provider::test_utils::MockEthProvider;
use reth_storage_api::noop::NoopProvider;
use reth_tracing::init_test_tracing;
use reth_transaction_pool::test_utils::testing_pool;
use std::time::Duration;

#[tokio::test(flavor = "multi_thread")]
async fn test_establish_connections() {
    reth_tracing::init_test_tracing();

    for _ in 0..3 {
        let handle = Testnet::create(3).await.spawn();
        let [peer0, peer1, peer2] = handle.peers_array();

        let listener0 = peer0.event_listener();
        let mut listener1 = peer1.event_listener();
        let mut listener2 = peer2.event_listener();

        peer0.add_peer(peer1);
        peer0.add_peer(peer2);

        let mut expected_connections = HashSet::from([*peer1.peer_id(), *peer2.peer_id()]);
        let mut expected_peers = expected_connections.clone();

        // wait for all initiator connections
        let mut established = listener0.take(4);
        while let Some(ev) = established.next().await {
            match ev {
                NetworkEvent::Peer(PeerEvent::SessionClosed { .. } | PeerEvent::PeerRemoved(_)) => {
                    panic!("unexpected event")
                }
                NetworkEvent::ActivePeerSession { info, .. } |
                NetworkEvent::Peer(PeerEvent::SessionEstablished(info)) => {
                    let SessionInfo { peer_id, .. } = info;
                    assert!(expected_connections.remove(&peer_id));
                }
                NetworkEvent::Peer(PeerEvent::PeerAdded(peer_id)) => {
                    assert!(expected_peers.remove(&peer_id))
                }
            }
        }
        assert!(expected_connections.is_empty());
        assert!(expected_peers.is_empty());

        // also await the established session on both target
        futures::future::join(listener1.next(), listener2.next()).await;

        let net = handle.terminate().await;

        assert_eq!(net.peers()[0].num_peers(), 2);
        assert_eq!(net.peers()[1].num_peers(), 1);
        assert_eq!(net.peers()[2].num_peers(), 1);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_already_connected() {
    reth_tracing::init_test_tracing();

    // initialize two peers with the same identifier
    let secret_key = rng_secret_key();
    let net = Testnet::from_configs([
        PeerConfig::default(),
        PeerConfig::default().with_secret_key(secret_key),
        PeerConfig::default().with_secret_key(secret_key),
    ])
    .await
    .spawn();
    let [peer0, peer1, peer2] = net.peers_array();

    let mut listener0 = peer0.event_stream();
    let mut listener2 = peer2.event_stream();

    peer0.add_peer(peer1);
    assert_eq!(listener0.next_session_established().await, Some(*peer1.peer_id()));

    peer2.add_peer(peer0);
    assert_eq!(listener2.next_session_established().await, Some(*peer0.peer_id()));

    let (peer, reason) = listener2.next_session_closed().await.unwrap();
    assert_eq!(peer, *peer0.peer_id());
    assert_eq!(reason, Some(DisconnectReason::AlreadyConnected));

    assert_eq!(peer0.network().num_connected_peers(), 1);
    assert_eq!(peer1.network().num_connected_peers(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_get_peer() {
    reth_tracing::init_test_tracing();

    let net = Testnet::create(3).await.spawn();
    let [peer0, peer1, peer2] = net.peers_array();
    let mut listener0 = peer0.event_stream();

    peer0.add_peer(peer1);
    listener0.next_session_established().await.unwrap();

    peer0.add_peer(peer2);
    listener0.next_session_established().await.unwrap();

    let peers = peer0.network().get_all_peers().await.unwrap();
    assert_eq!(peer0.network().num_connected_peers(), peers.len());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_get_peer_by_id() {
    reth_tracing::init_test_tracing();

    let net = Testnet::create(3).await.spawn();
    let [peer0, peer1, peer2] = net.peers_array();
    let mut listener0 = peer0.event_stream();

    peer0.add_peer(peer1);
    listener0.next_session_established().await.unwrap();

    let peer = peer0.network().get_peer_by_id(*peer1.peer_id()).await.unwrap();
    assert!(peer.is_some());

    let peer = peer0.network().get_peer_by_id(*peer2.peer_id()).await.unwrap();
    assert!(peer.is_none());
}

// expects a `ENODE="enode://"` env var that holds the record
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn test_connect_to_trusted_peer() {
    reth_tracing::init_test_tracing();

    let mut net = Testnet::create(1).await.with_request_handlers();
    net.peers_mut()[0].install_transactions_manager(testing_pool());
    let net = net.spawn();
    let [peer] = net.peers_array();
    let mut events = peer.event_listener();

    let node: NodeRecord = std::env::var("ENODE").unwrap().parse().unwrap();
    peer.network().add_trusted_peer(node.id, node.tcp_addr());
    peer.network().update_sync_state(SyncState::Syncing);

    let fetcher = peer.network().fetch_client().await.unwrap();
    let headers = fetcher
        .get_headers(HeadersRequest {
            start: 73174u64.into(),
            limit: 10,
            direction: HeadersDirection::Falling,
        })
        .await;
    dbg!(&headers);

    while let Some(ev) = events.next().await {
        dbg!(ev);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_shutdown() {
    reth_tracing::init_test_tracing();

    let net = Testnet::create(3).await.spawn();
    let [peer0, peer1, peer2] = net.peers_array();
    let mut listener0 = peer0.event_stream();
    let mut listener1 = peer1.event_stream();

    peer0.add_peer(peer1);
    peer0.add_peer(peer2);
    peer1.add_peer(peer2);

    let mut expected_connections = HashSet::from([*peer1.peer_id(), *peer2.peer_id()]);

    // Before shutting down, we have two connected peers
    let established1 = listener0.next_session_established().await.unwrap();
    let established2 = listener0.next_session_established().await.unwrap();
    assert_eq!(peer0.network().num_connected_peers(), 2);
    assert!(expected_connections.contains(&established1));
    assert!(expected_connections.contains(&established2));

    peer0.network().shutdown().await.unwrap();

    // All sessions get disconnected
    let (closed1, _reason) = listener0.next_session_closed().await.unwrap();
    let (closed2, _reason) = listener0.next_session_closed().await.unwrap();
    assert_eq!(peer0.network().num_connected_peers(), 0);
    assert!(expected_connections.remove(&closed1));
    assert!(expected_connections.remove(&closed2));

    // Connected peers receive a shutdown signal
    let (_peer, reason) = listener1.next_session_closed().await.unwrap();
    assert_eq!(reason, Some(DisconnectReason::ClientQuitting));

    // New connections ignored
    peer0.add_peer(peer1);
    assert_eq!(peer0.network().num_connected_peers(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_trusted_peer_only() {
    init_test_tracing();

    let net = Testnet::from_configs([
        PeerConfig::default(),
        PeerConfig::default(),
        PeerConfig::default().with_peers_config(PeersConfig::test().with_trusted_nodes_only(true)),
    ])
    .await
    .spawn();
    // `peer` only accepts trusted peers, and:
    // * peer0 is used to test that outgoing connections to untrusted peers are not allowed, and
    //   outgoing connections to trusted peers are allowed and succeed
    // * peer1 is used to test that incoming connections from untrusted peers are not allowed, and
    //   incoming connections from trusted peers are allowed and succeed
    let [peer0, peer1, peer] = net.peers_array();
    let mut event_stream = peer.event_stream();

    // connect to an untrusted peer should fail.
    peer.add_peer(peer0);

    // wait 500ms, the number of connection is still 0.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(peer.network().num_connected_peers(), 0);

    // add to trusted peer.
    peer.add_trusted_peer(peer0);

    assert_eq!(event_stream.next_session_established().await, Some(*peer0.peer_id()));
    assert_eq!(peer.network().num_connected_peers(), 1);

    // only receive connections from trusted peers.
    peer1.add_peer(peer);

    // wait 500ms, the number of connections is still 1, because peer1 is untrusted.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(peer.network().num_connected_peers(), 1);

    // remove peer from peer1's peer list to prevent a competing outgoing connection attempt
    // from peer1 racing with peer's outgoing connection below, which can cause duplicate
    // session resolution to drop a connection
    peer1.network().remove_peer(*peer.peer_id(), PeerKind::Basic);

    peer.add_trusted_peer(peer1);

    // wait for the next session established event to check the peer1 incoming connection
    assert_eq!(event_stream.next_session_established().await, Some(*peer1.peer_id()));

    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(peer.network().num_connected_peers(), 2);

    // check that peer0 and peer1 both have peers.
    assert_eq!(peer0.network().num_connected_peers(), 1);
    assert_eq!(peer1.network().num_connected_peers(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_network_state_change() {
    let net = Testnet::create(2).await.spawn();
    let [peer0, peer] = net.peers_array();
    let mut event_stream = peer.event_stream();

    // Set network state to Hibernate.
    peer.network().set_network_hibernate();

    peer.add_peer(peer0);

    // wait 500ms, the number of connections is still 0, because network is Hibernate.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(peer.network().num_connected_peers(), 0);

    // Set network state to Active.
    peer.network().set_network_active();

    // the outbound slot should be filled now that the network is Active.
    assert_eq!(event_stream.next_session_established().await, Some(*peer0.peer_id()));
    assert_eq!(peer.network().num_connected_peers(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_exceed_outgoing_connections() {
    let net = Testnet::from_configs([
        PeerConfig::default(),
        PeerConfig::default(),
        PeerConfig::default().with_peers_config(PeersConfig::test().with_max_outbound(1)),
    ])
    .await
    .spawn();
    let [peer0, peer1, peer] = net.peers_array();
    let mut event_stream = peer.event_stream();

    peer.add_peer(peer0);
    assert_eq!(event_stream.next_session_established().await, Some(*peer0.peer_id()));

    peer.add_peer(peer1);

    // wait 500ms, the number of connections is still 1, indicating that the max outbound is in
    // effect.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(peer.network().num_connected_peers(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_disconnect_incoming_when_exceeded_incoming_connections() {
    let net = Testnet::from_configs([PeerConfig::default(), max_inbound(0)]).await.spawn();
    let [other_peer, peer] = net.peers_array();

    other_peer.add_peer(peer);

    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(peer.network().num_connected_peers(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_always_accept_incoming_connections_from_trusted_peers() {
    reth_tracing::init_test_tracing();

    let mut net = Testnet::from_configs([max_inbound(10), max_inbound(0)]).await;

    // setup the peer with max_inbound = 0, and add peer2 as trusted node
    let trusted_peer2 = TrustedPeer::from(net.peers()[1].handle().local_node_record());
    let peer_config =
        PeersConfig::test().with_max_inbound(0).with_trusted_nodes(vec![trusted_peer2]);
    net.add_peer_with_config(PeerConfig::default().with_peers_config(peer_config)).await.unwrap();

    let net = net.spawn();
    let [peer1, peer2, peer] = net.peers_array();
    let mut events = peer.event_stream();
    let mut events_peer1 = peer1.event_stream();

    // incoming connection should fail because exceeding max_inbound
    peer1.add_peer(peer);

    let (peer_id, reason) = events_peer1.next_session_closed().await.unwrap();
    assert_eq!(peer_id, *peer.peer_id());
    assert_eq!(reason, Some(DisconnectReason::TooManyPeers));

    assert_eq!(events.next_session_established().await, Some(*peer1.peer_id()));

    // outbound connection from `peer2` should succeed
    peer2.add_peer(peer);
    assert_eq!(events.next_session_established().await, Some(*peer2.peer_id()));

    assert_eq!(peer.network().num_connected_peers(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_rejected_by_already_connect() {
    reth_tracing::init_test_tracing();

    //  setup the peer with max_inbound = 2
    let net =
        Testnet::from_configs([max_inbound(10), max_inbound(10), max_inbound(2)]).await.spawn();
    let [other_peer1, other_peer2, peer] = net.peers_array();
    let mut events = peer.event_stream();

    // incoming connection should succeed
    other_peer1.add_peer(peer);
    assert_eq!(events.next_session_established().await, Some(*other_peer1.peer_id()));
    assert_eq!(peer.network().num_connected_peers(), 1);

    // incoming connection from the same peer should be rejected by already connected
    // and num_inbount should still be 1
    other_peer1.add_peer(peer);
    tokio::time::sleep(Duration::from_millis(500)).await;

    // incoming connection from other_peer2 should succeed
    other_peer2.add_peer(peer);
    assert_eq!(events.next_session_established().await, Some(*other_peer2.peer_id()));

    // wait 500ms and check that other_peer2 is not rejected by TooManyPeers
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(peer.network().num_connected_peers(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_connect_many() {
    reth_tracing::init_test_tracing();

    let provider = MockEthProvider::default().with_genesis_block();
    let net = Testnet::create_with(5, provider).await.with_eth_pool().spawn();
    // connect all the peers
    net.connect_peers().await;

    // check that all the peers are connected
    for peer in net.peers() {
        assert_eq!(peer.network().num_connected_peers(), 4);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_disconnect_then_connect() {
    reth_tracing::init_test_tracing();

    let net = Testnet::create(2).await.spawn();
    let [peer0, peer1] = net.peers_array();
    let mut listener0 = peer0.event_stream();

    peer0.add_peer(peer1);
    assert_eq!(listener0.next_session_established().await, Some(*peer1.peer_id()));

    peer0.network().disconnect_peer(*peer1.peer_id());

    let (peer, _) = listener0.next_session_closed().await.unwrap();
    assert_eq!(peer, *peer1.peer_id());

    peer0.network().connect_peer(*peer1.peer_id(), peer1.local_addr());
    assert_eq!(listener0.next_session_established().await, Some(*peer1.peer_id()));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_connect_peer_in_different_network_should_fail() {
    reth_tracing::init_test_tracing();

    let net = Testnet::from_configs([
        // peer in mainnet.
        max_inbound(10),
        // peer in sepolia. If the remote disconnect first, then we would not get a fatal protocol
        // error. So set max_backoff_count to 0 to speed up the removal of the peer.
        PeerConfig::new(NoopProvider::eth(SEPOLIA.clone()))
            .with_peers_config(PeersConfig::default().with_max_backoff_count(0)),
    ])
    .await
    .spawn();
    let [mainnet_peer, sepolia_peer] = net.peers_array();
    let mut event_stream = sepolia_peer.event_stream();

    sepolia_peer.add_peer(mainnet_peer);

    assert_eq!(event_stream.peer_added().await, Some(*mainnet_peer.peer_id()));
    assert_eq!(event_stream.peer_removed().await, Some(*mainnet_peer.peer_id()));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_reconnect_trusted() {
    reth_tracing::init_test_tracing();

    let net = Testnet::create(2).await.spawn();
    let [peer0, peer1] = net.peers_array();
    let mut listener0 = peer0.event_stream();

    // Connect the two peers
    peer0.add_peer(peer1);
    peer1.add_peer(peer0);
    assert_eq!(listener0.next_session_established().await, Some(*peer1.peer_id()));
    assert_eq!(peer0.network().num_connected_peers(), 1);

    // Add peer1 as a trusted peer
    peer0.add_trusted_peer(peer1);

    // Trigger disconnect from peer0
    peer0.network().disconnect_peer(*peer1.peer_id());

    // Wait for the session to close
    let (peer, _) = listener0.next_session_closed().await.unwrap();
    assert_eq!(peer, *peer1.peer_id());
    assert_eq!(peer0.network().num_connected_peers(), 0);

    // Await that peer1 (trusted peer) reconnects automatically
    let reconnected =
        tokio::time::timeout(Duration::from_secs(10), listener0.next_session_established())
            .await
            .expect("trusted peer did not reconnect in time");
    assert_eq!(reconnected, Some(*peer1.peer_id()));
    assert_eq!(peer0.network().num_connected_peers(), 1);
}

/// A peer that accepts at most `max_inbound` incoming connections.
fn max_inbound(max_inbound: usize) -> PeerConfig {
    PeerConfig::default().with_peers_config(PeersConfig::test().with_max_inbound(max_inbound))
}

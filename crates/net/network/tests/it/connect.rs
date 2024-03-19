//! Connection tests

use alloy_node_bindings::Geth;
use ethers_providers::{Http, Middleware, Provider};
use futures::StreamExt;
use reth_discv4::Discv4Config;
use reth_eth_wire::DisconnectReason;
use reth_interfaces::{
    p2p::headers::client::{HeadersClient, HeadersRequest},
    sync::{NetworkSyncUpdater, SyncState},
};
use reth_net_common::ban_list::BanList;
use reth_network::{
    test_utils::{enr_to_peer_id, NetworkEventStream, PeerConfig, Testnet, GETH_TIMEOUT},
    NetworkConfigBuilder, NetworkEvent, NetworkEvents, NetworkManager, PeersConfig,
};
use reth_network_api::{NetworkInfo, Peers, PeersInfo};
use reth_primitives::{mainnet_nodes, HeadersDirection, NodeRecord, PeerId};
use reth_provider::test_utils::NoopProvider;
use reth_transaction_pool::test_utils::testing_pool;
use secp256k1::SecretKey;
use std::{collections::HashSet, net::SocketAddr, time::Duration};
use tokio::task;

#[tokio::test(flavor = "multi_thread")]
async fn test_establish_connections() {
    reth_tracing::init_test_tracing();

    for _ in 0..3 {
        let net = Testnet::create(3).await;

        net.for_each(|peer| assert_eq!(0, peer.num_peers()));

        let mut handles = net.handles();
        let handle0 = handles.next().unwrap();
        let handle1 = handles.next().unwrap();
        let handle2 = handles.next().unwrap();

        drop(handles);
        let handle = net.spawn();

        let listener0 = handle0.event_listener();

        let mut listener1 = handle1.event_listener();
        let mut listener2 = handle2.event_listener();

        handle0.add_peer(*handle1.peer_id(), handle1.local_addr());
        handle0.add_peer(*handle2.peer_id(), handle2.local_addr());

        let mut expected_connections = HashSet::from([*handle1.peer_id(), *handle2.peer_id()]);
        let mut expected_peers = expected_connections.clone();

        // wait for all initiator connections
        let mut established = listener0.take(4);
        while let Some(ev) = established.next().await {
            match ev {
                NetworkEvent::SessionClosed { .. } => {
                    panic!("unexpected event")
                }
                NetworkEvent::SessionEstablished { peer_id, .. } => {
                    assert!(expected_connections.remove(&peer_id))
                }
                NetworkEvent::PeerAdded(peer_id) => {
                    assert!(expected_peers.remove(&peer_id))
                }
                NetworkEvent::PeerRemoved(_) => {
                    panic!("unexpected event")
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
    let mut net = Testnet::default();

    let secret_key = SecretKey::new(&mut rand::thread_rng());
    let client = NoopProvider::default();
    let p1 = PeerConfig::default();

    // initialize two peers with the same identifier
    let p2 = PeerConfig::with_secret_key(client, secret_key);
    let p3 = PeerConfig::with_secret_key(client, secret_key);

    net.extend_peer_with_config(vec![p1, p2, p3]).await.unwrap();

    let mut handles = net.handles();
    let handle0 = handles.next().unwrap();
    let handle1 = handles.next().unwrap();
    let handle2 = handles.next().unwrap();

    drop(handles);
    let _handle = net.spawn();

    let mut listener0 = NetworkEventStream::new(handle0.event_listener());
    let mut listener2 = NetworkEventStream::new(handle2.event_listener());

    handle0.add_peer(*handle1.peer_id(), handle1.local_addr());

    let peer = listener0.next_session_established().await.unwrap();
    assert_eq!(peer, *handle1.peer_id());

    handle2.add_peer(*handle0.peer_id(), handle0.local_addr());
    let peer = listener2.next_session_established().await.unwrap();
    assert_eq!(peer, *handle0.peer_id());

    let (peer, reason) = listener2.next_session_closed().await.unwrap();
    assert_eq!(peer, *handle0.peer_id());
    let reason = reason.unwrap();
    assert_eq!(reason, DisconnectReason::AlreadyConnected);

    assert_eq!(handle0.num_connected_peers(), 1);
    assert_eq!(handle1.num_connected_peers(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_get_peer() {
    reth_tracing::init_test_tracing();

    let mut net = Testnet::default();
    let secret_key = SecretKey::new(&mut rand::thread_rng());
    let secret_key_1 = SecretKey::new(&mut rand::thread_rng());
    let client = NoopProvider::default();

    let p1 = PeerConfig::default();
    let p2 = PeerConfig::with_secret_key(client, secret_key);
    let p3 = PeerConfig::with_secret_key(client, secret_key_1);
    net.extend_peer_with_config(vec![p1, p2, p3]).await.unwrap();

    let mut handles = net.handles();
    let handle0 = handles.next().unwrap();
    let handle1 = handles.next().unwrap();
    let handle2 = handles.next().unwrap();

    drop(handles);
    let _handle = net.spawn();

    let mut listener0 = NetworkEventStream::new(handle0.event_listener());

    handle0.add_peer(*handle1.peer_id(), handle1.local_addr());
    let _ = listener0.next_session_established().await.unwrap();

    handle0.add_peer(*handle2.peer_id(), handle2.local_addr());
    let _ = listener0.next_session_established().await.unwrap();

    let peers = handle0.get_all_peers().await.unwrap();
    assert_eq!(handle0.num_connected_peers(), peers.len());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_get_peer_by_id() {
    reth_tracing::init_test_tracing();
    let mut net = Testnet::default();

    let secret_key = SecretKey::new(&mut rand::thread_rng());
    let secret_key_1 = SecretKey::new(&mut rand::thread_rng());
    let client = NoopProvider::default();
    let p1 = PeerConfig::default();
    let p2 = PeerConfig::with_secret_key(client, secret_key);
    let p3 = PeerConfig::with_secret_key(client, secret_key_1);

    net.extend_peer_with_config(vec![p1, p2, p3]).await.unwrap();

    let mut handles = net.handles();
    let handle0 = handles.next().unwrap();
    let handle1 = handles.next().unwrap();
    let handle2 = handles.next().unwrap();

    drop(handles);
    let _handle = net.spawn();

    let mut listener0 = NetworkEventStream::new(handle0.event_listener());

    handle0.add_peer(*handle1.peer_id(), handle1.local_addr());
    let _ = listener0.next_session_established().await.unwrap();

    let peer = handle0.get_peer_by_id(*handle1.peer_id()).await.unwrap();
    assert!(peer.is_some());

    let peer = handle0.get_peer_by_id(*handle2.peer_id()).await.unwrap();
    assert!(peer.is_none());
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn test_connect_with_boot_nodes() {
    reth_tracing::init_test_tracing();
    let secret_key = SecretKey::new(&mut rand::thread_rng());
    let mut discv4 = Discv4Config::builder();
    discv4.add_boot_nodes(mainnet_nodes());

    let config =
        NetworkConfigBuilder::new(secret_key).discovery(discv4).build(NoopProvider::default());
    let network = NetworkManager::new(config).await.unwrap();

    let handle = network.handle().clone();
    let mut events = handle.event_listener();
    tokio::task::spawn(network);

    while let Some(ev) = events.next().await {
        dbg!(ev);
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn test_connect_with_builder() {
    reth_tracing::init_test_tracing();
    let secret_key = SecretKey::new(&mut rand::thread_rng());
    let mut discv4 = Discv4Config::builder();
    discv4.add_boot_nodes(mainnet_nodes());

    let client = NoopProvider::default();
    let config = NetworkConfigBuilder::new(secret_key).discovery(discv4).build(client);
    let (handle, network, _, requests) = NetworkManager::new(config)
        .await
        .unwrap()
        .into_builder()
        .request_handler(client)
        .split_with_handle();

    let mut events = handle.event_listener();

    tokio::task::spawn(async move {
        tokio::join!(network, requests);
    });

    let h = handle.clone();
    task::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            dbg!(h.num_connected_peers());
        }
    });

    while let Some(ev) = events.next().await {
        dbg!(ev);
    }
}

// expects a `ENODE="enode://"` env var that holds the record
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn test_connect_to_trusted_peer() {
    reth_tracing::init_test_tracing();
    let secret_key = SecretKey::new(&mut rand::thread_rng());
    let discv4 = Discv4Config::builder();

    let client = NoopProvider::default();
    let config = NetworkConfigBuilder::new(secret_key).discovery(discv4).build(client);
    let transactions_manager_config = config.transactions_manager_config.clone();
    let (handle, network, transactions, requests) = NetworkManager::new(config)
        .await
        .unwrap()
        .into_builder()
        .request_handler(client)
        .transactions(testing_pool(), transactions_manager_config)
        .split_with_handle();

    let mut events = handle.event_listener();

    tokio::task::spawn(async move {
        tokio::join!(network, requests, transactions);
    });

    let node: NodeRecord = std::env::var("ENODE").unwrap().parse().unwrap();

    handle.add_trusted_peer(node.id, node.tcp_addr());

    let h = handle.clone();
    h.update_sync_state(SyncState::Syncing);

    task::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            dbg!(h.num_connected_peers());
        }
    });

    let fetcher = handle.fetch_client().await.unwrap();

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
#[serial_test::serial]
#[cfg_attr(not(feature = "geth-tests"), ignore)]
async fn test_incoming_node_id_blacklist() {
    reth_tracing::init_test_tracing();
    tokio::time::timeout(GETH_TIMEOUT, async move {
        let secret_key = SecretKey::new(&mut rand::thread_rng());

        // instantiate geth and add ourselves as a peer
        let temp_dir = tempfile::tempdir().unwrap().into_path();
        let geth = Geth::new().data_dir(temp_dir).disable_discovery().authrpc_port(0).spawn();
        let geth_endpoint = SocketAddr::new([127, 0, 0, 1].into(), geth.port());
        let provider = Provider::<Http>::try_from(format!("http://{geth_endpoint}")).unwrap();

        // get the peer id we should be expecting
        let geth_peer_id = enr_to_peer_id(provider.node_info().await.unwrap().enr);

        let ban_list = BanList::new(vec![geth_peer_id], HashSet::new());
        let peer_config = PeersConfig::default().with_ban_list(ban_list);

        let config = NetworkConfigBuilder::new(secret_key)
            .listener_port(0)
            .disable_discovery()
            .peer_config(peer_config)
            .build(NoopProvider::default());

        let network = NetworkManager::new(config).await.unwrap();

        let handle = network.handle().clone();
        let events = handle.event_listener();

        tokio::task::spawn(network);

        // make geth connect to us
        let our_enode = NodeRecord::new(handle.local_addr(), *handle.peer_id());

        provider.add_peer(our_enode.to_string()).await.unwrap();

        let mut event_stream = NetworkEventStream::new(events);

        // check for session to be opened
        let incoming_peer_id = event_stream.next_session_established().await.unwrap();
        assert_eq!(incoming_peer_id, geth_peer_id);

        // check to see that the session was closed
        let incoming_peer_id = event_stream.next_session_closed().await.unwrap().0;
        assert_eq!(incoming_peer_id, geth_peer_id);
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
#[cfg_attr(not(feature = "geth-tests"), ignore)]
async fn test_incoming_connect_with_single_geth() {
    reth_tracing::init_test_tracing();
    tokio::time::timeout(GETH_TIMEOUT, async move {
        let secret_key = SecretKey::new(&mut rand::thread_rng());

        // instantiate geth and add ourselves as a peer
        let temp_dir = tempfile::tempdir().unwrap().into_path();
        let geth = Geth::new().data_dir(temp_dir).disable_discovery().authrpc_port(0).spawn();
        let geth_endpoint = SocketAddr::new([127, 0, 0, 1].into(), geth.port());
        let provider = Provider::<Http>::try_from(format!("http://{geth_endpoint}")).unwrap();

        // get the peer id we should be expecting
        let geth_peer_id = enr_to_peer_id(provider.node_info().await.unwrap().enr);

        let config = NetworkConfigBuilder::new(secret_key)
            .listener_port(0)
            .disable_discovery()
            .build(NoopProvider::default());

        let network = NetworkManager::new(config).await.unwrap();

        let handle = network.handle().clone();
        tokio::task::spawn(network);

        let events = handle.event_listener();
        let mut event_stream = NetworkEventStream::new(events);

        // make geth connect to us
        let our_enode = NodeRecord::new(handle.local_addr(), *handle.peer_id());

        provider.add_peer(our_enode.to_string()).await.unwrap();

        // check for a sessionestablished event
        let incoming_peer_id = event_stream.next_session_established().await.unwrap();
        assert_eq!(incoming_peer_id, geth_peer_id);
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
#[cfg_attr(not(feature = "geth-tests"), ignore)]
async fn test_outgoing_connect_with_single_geth() {
    reth_tracing::init_test_tracing();
    tokio::time::timeout(GETH_TIMEOUT, async move {
        let secret_key = SecretKey::new(&mut rand::thread_rng());

        let config = NetworkConfigBuilder::new(secret_key)
            .listener_port(0)
            .disable_discovery()
            .build(NoopProvider::default());
        let network = NetworkManager::new(config).await.unwrap();

        let handle = network.handle().clone();
        tokio::task::spawn(network);

        // create networkeventstream to get the next session established event easily
        let events = handle.event_listener();
        let mut event_stream = NetworkEventStream::new(events);

        // instantiate geth and add ourselves as a peer
        let temp_dir = tempfile::tempdir().unwrap().into_path();
        let geth = Geth::new().disable_discovery().data_dir(temp_dir).authrpc_port(0).spawn();

        let geth_p2p_port = geth.p2p_port().unwrap();
        let geth_socket = SocketAddr::new([127, 0, 0, 1].into(), geth_p2p_port);
        let geth_endpoint = SocketAddr::new([127, 0, 0, 1].into(), geth.port()).to_string();

        let provider = Provider::<Http>::try_from(format!("http://{geth_endpoint}")).unwrap();

        // get the peer id we should be expecting
        let geth_peer_id: PeerId = enr_to_peer_id(provider.node_info().await.unwrap().enr);

        // add geth as a peer then wait for a `SessionEstablished` event
        handle.add_peer(geth_peer_id, geth_socket);

        // check for a sessionestablished event
        let incoming_peer_id = event_stream.next_session_established().await.unwrap();
        assert_eq!(incoming_peer_id, geth_peer_id);
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
#[serial_test::serial]
#[cfg_attr(not(feature = "geth-tests"), ignore)]
async fn test_geth_disconnect() {
    reth_tracing::init_test_tracing();
    tokio::time::timeout(GETH_TIMEOUT, async move {
        let secret_key = SecretKey::new(&mut rand::thread_rng());

        let config = NetworkConfigBuilder::new(secret_key)
            .listener_port(0)
            .disable_discovery()
            .build(NoopProvider::default());
        let network = NetworkManager::new(config).await.unwrap();

        let handle = network.handle().clone();
        tokio::task::spawn(network);

        // create networkeventstream to get the next session established event easily
        let mut events = handle.event_listener();

        // instantiate geth and add ourselves as a peer
        let temp_dir = tempfile::tempdir().unwrap().into_path();
        let geth = Geth::new().disable_discovery().data_dir(temp_dir).authrpc_port(0).spawn();

        let geth_p2p_port = geth.p2p_port().unwrap();
        let geth_socket = SocketAddr::new([127, 0, 0, 1].into(), geth_p2p_port);
        let geth_endpoint = SocketAddr::new([127, 0, 0, 1].into(), geth.port()).to_string();

        let provider = Provider::<Http>::try_from(format!("http://{geth_endpoint}")).unwrap();

        // get the peer id we should be expecting
        let geth_peer_id: PeerId = enr_to_peer_id(provider.node_info().await.unwrap().enr);

        // add geth as a peer then wait for `PeerAdded` and `SessionEstablished` events.
        handle.add_peer(geth_peer_id, geth_socket);

        match events.next().await {
            Some(NetworkEvent::PeerAdded(peer_id)) => assert_eq!(peer_id, geth_peer_id),
            _ => panic!("Expected a peer added event"),
        }

        if let Some(NetworkEvent::SessionEstablished { peer_id, .. }) = events.next().await {
            assert_eq!(peer_id, geth_peer_id);
        } else {
            panic!("Expected a session established event");
        }

        // remove geth as a peer deliberately
        handle.disconnect_peer(geth_peer_id);

        // wait for a disconnect from geth
        if let Some(NetworkEvent::SessionClosed { peer_id, .. }) = events.next().await {
            assert_eq!(peer_id, geth_peer_id);
        } else {
            panic!("Expected a session closed event");
        }
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_shutdown() {
    reth_tracing::init_test_tracing();
    let net = Testnet::create(3).await;

    let mut handles = net.handles();
    let handle0 = handles.next().unwrap();
    let handle1 = handles.next().unwrap();
    let handle2 = handles.next().unwrap();

    drop(handles);
    let _handle = net.spawn();

    let mut listener0 = NetworkEventStream::new(handle0.event_listener());
    let mut listener1 = NetworkEventStream::new(handle1.event_listener());

    handle0.add_peer(*handle1.peer_id(), handle1.local_addr());
    handle0.add_peer(*handle2.peer_id(), handle2.local_addr());
    handle1.add_peer(*handle2.peer_id(), handle2.local_addr());

    let mut expected_connections = HashSet::from([*handle1.peer_id(), *handle2.peer_id()]);

    // Before shutting down, we have two connected peers
    let peer1 = listener0.next_session_established().await.unwrap();
    let peer2 = listener0.next_session_established().await.unwrap();
    assert_eq!(handle0.num_connected_peers(), 2);
    assert!(expected_connections.contains(&peer1));
    assert!(expected_connections.contains(&peer2));

    handle0.shutdown().await.unwrap();

    // All sessions get disconnected
    let (peer1, _reason) = listener0.next_session_closed().await.unwrap();
    let (peer2, _reason) = listener0.next_session_closed().await.unwrap();
    assert_eq!(handle0.num_connected_peers(), 0);
    assert!(expected_connections.remove(&peer1));
    assert!(expected_connections.remove(&peer2));

    // Connected peers receive a shutdown signal
    let (_peer, reason) = listener1.next_session_closed().await.unwrap();
    assert_eq!(reason, Some(DisconnectReason::ClientQuitting));

    // New connections ignored
    handle0.add_peer(*handle1.peer_id(), handle1.local_addr());
    assert_eq!(handle0.num_connected_peers(), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_disconnect_incoming_when_exceeded_incoming_connections() {
    let net = Testnet::create(1).await;
    let secret_key = SecretKey::new(&mut rand::thread_rng());
    let peers_config = PeersConfig::default().with_max_inbound(0);

    let config = NetworkConfigBuilder::new(secret_key)
        .listener_port(0)
        .disable_discovery()
        .peer_config(peers_config)
        .build(NoopProvider::default());

    let network = NetworkManager::new(config).await.unwrap();

    let other_peer_handle = net.handles().next().unwrap();

    let handle = network.handle().clone();

    other_peer_handle.add_peer(*handle.peer_id(), handle.local_addr());

    tokio::task::spawn(network);
    let net_handle = net.spawn();

    tokio::time::sleep(Duration::from_secs(1)).await;

    assert_eq!(handle.num_connected_peers(), 0);

    net_handle.terminate().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_always_accept_incoming_connections_from_trusted_peers() {
    reth_tracing::init_test_tracing();
    let peer1 = new_random_peer(10, HashSet::new()).await;
    let peer2 = new_random_peer(0, HashSet::new()).await;

    //  setup the peer with max_inbound = 1, and add other_peer_3 as trust nodes
    let peer =
        new_random_peer(0, HashSet::from([NodeRecord::new(peer2.local_addr(), *peer2.peer_id())]))
            .await;

    let handle = peer.handle().clone();
    let peer1_handle = peer1.handle().clone();
    let peer2_handle = peer2.handle().clone();

    tokio::task::spawn(peer);
    tokio::task::spawn(peer1);
    tokio::task::spawn(peer2);

    let mut events = NetworkEventStream::new(handle.event_listener());
    let mut events_peer1 = NetworkEventStream::new(peer1_handle.event_listener());

    // incoming connection should fail because exceeding max_inbound
    peer1_handle.add_peer(*handle.peer_id(), handle.local_addr());

    let (peer_id, reason) = events_peer1.next_session_closed().await.unwrap();
    assert_eq!(peer_id, *handle.peer_id());
    assert_eq!(reason, Some(DisconnectReason::TooManyPeers));

    let peer_id = events.next_session_established().await.unwrap();
    assert_eq!(peer_id, *peer1_handle.peer_id());

    // outbound connection from `peer2` should succeed
    peer2_handle.add_peer(*handle.peer_id(), handle.local_addr());
    let peer_id = events.next_session_established().await.unwrap();
    assert_eq!(peer_id, *peer2_handle.peer_id());

    assert_eq!(handle.num_connected_peers(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_rejected_by_already_connect() {
    reth_tracing::init_test_tracing();
    let other_peer1 = new_random_peer(10, HashSet::new()).await;
    let other_peer2 = new_random_peer(10, HashSet::new()).await;

    //  setup the peer with max_inbound = 2
    let peer = new_random_peer(2, HashSet::new()).await;

    let handle = peer.handle().clone();
    let other_peer_handle1 = other_peer1.handle().clone();
    let other_peer_handle2 = other_peer2.handle().clone();

    tokio::task::spawn(peer);
    tokio::task::spawn(other_peer1);
    tokio::task::spawn(other_peer2);

    let mut events = NetworkEventStream::new(handle.event_listener());

    // incoming connection should succeed
    other_peer_handle1.add_peer(*handle.peer_id(), handle.local_addr());
    let peer_id = events.next_session_established().await.unwrap();
    assert_eq!(peer_id, *other_peer_handle1.peer_id());
    assert_eq!(handle.num_connected_peers(), 1);

    // incoming connection from the same peer should be rejected by already connected
    // and num_inbount should still be 1
    other_peer_handle1.add_peer(*handle.peer_id(), handle.local_addr());
    tokio::time::sleep(Duration::from_secs(1)).await;

    // incoming connection from other_peer2 should succeed
    other_peer_handle2.add_peer(*handle.peer_id(), handle.local_addr());
    let peer_id = events.next_session_established().await.unwrap();
    assert_eq!(peer_id, *other_peer_handle2.peer_id());

    // wait 2 seconds and check that other_peer2 is not rejected by TooManyPeers
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(handle.num_connected_peers(), 2);
}

async fn new_random_peer(
    max_in_bound: usize,
    trusted_nodes: HashSet<NodeRecord>,
) -> NetworkManager<NoopProvider> {
    let secret_key = SecretKey::new(&mut rand::thread_rng());
    let peers_config =
        PeersConfig::default().with_max_inbound(max_in_bound).with_trusted_nodes(trusted_nodes);

    let config = NetworkConfigBuilder::new(secret_key)
        .listener_port(0)
        .disable_discovery()
        .peer_config(peers_config)
        .build(NoopProvider::default());

    NetworkManager::new(config).await.unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn test_connect_many() {
    reth_tracing::init_test_tracing();

    let net = Testnet::create_with(5, NoopProvider::default()).await;

    // install request handlers
    let net = net.with_eth_pool();
    let handle = net.spawn();
    // connect all the peers
    handle.connect_peers().await;

    // check that all the peers are connected
    for peer in handle.peers() {
        assert_eq!(peer.network().num_connected_peers(), 4);
    }
}

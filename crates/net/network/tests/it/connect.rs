//! Connection tests

use crate::NetworkEventStream;

use super::testnet::Testnet;
use enr::EnrPublicKey;
use ethers_core::utils::Geth;
use ethers_providers::{Http, Middleware, Provider};
use futures::StreamExt;
use reth_discv4::{bootnodes::mainnet_nodes, Discv4Config};
use reth_net_common::ban_list::BanList;
use reth_network::{NetworkConfig, NetworkEvent, NetworkManager, PeersConfig};
use reth_primitives::PeerId;
use reth_provider::test_utils::TestApi;
use secp256k1::SecretKey;
use std::{collections::HashSet, net::SocketAddr, sync::Arc, time::Duration};
use tokio::task;

#[tokio::test(flavor = "multi_thread")]
async fn test_establish_connections() {
    reth_tracing::init_tracing();

    for _ in 0..10 {
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

        // wait for all initiator connections
        let mut established = listener0.take(2);
        while let Some(ev) = established.next().await {
            match ev {
                NetworkEvent::SessionClosed { .. } => {
                    panic!("unexpected event")
                }
                NetworkEvent::SessionEstablished { peer_id, .. } => {
                    assert!(expected_connections.remove(&peer_id))
                }
            }
        }
        assert!(expected_connections.is_empty());

        // also await the established session on both target
        futures::future::join(listener1.next(), listener2.next()).await;

        let net = handle.terminate().await;

        assert_eq!(net.peers()[0].num_peers(), 2);
        assert_eq!(net.peers()[1].num_peers(), 1);
        assert_eq!(net.peers()[2].num_peers(), 1);
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn test_connect_with_boot_nodes() {
    reth_tracing::init_tracing();
    let secret_key = SecretKey::new(&mut rand::thread_rng());
    let mut discv4 = Discv4Config::builder();
    discv4.add_boot_nodes(mainnet_nodes());

    let config =
        NetworkConfig::builder(Arc::new(TestApi::default()), secret_key).discovery(discv4).build();
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
    reth_tracing::init_tracing();
    let secret_key = SecretKey::new(&mut rand::thread_rng());
    let mut discv4 = Discv4Config::builder();
    discv4.add_boot_nodes(mainnet_nodes());

    let client = Arc::new(TestApi::default());
    let config = NetworkConfig::builder(Arc::clone(&client), secret_key).discovery(discv4).build();
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

#[tokio::test(flavor = "multi_thread")]
async fn test_incoming_node_id_blacklist() {
    reth_tracing::init_tracing();
    let secret_key = SecretKey::new(&mut rand::thread_rng());

    // instantiate geth and add ourselves as a peer
    let temp_dir = tempfile::tempdir().unwrap().into_path();
    let geth = Geth::new().data_dir(temp_dir).disable_discovery().spawn();
    let geth_endpoint = SocketAddr::new([127, 0, 0, 1].into(), geth.port());
    let provider = Provider::<Http>::try_from(format!("http://{geth_endpoint}")).unwrap();

    // get the peer id we should be expecting
    let geth_peer_id: PeerId =
        provider.node_info().await.unwrap().enr.public_key().encode_uncompressed().into();

    let ban_list = BanList::new(vec![geth_peer_id], HashSet::new());
    let peer_config = PeersConfig::default().with_ban_list(ban_list);

    let reth_p2p_socket = SocketAddr::new([127, 0, 0, 1].into(), 30303);
    let reth_disc_socket = SocketAddr::new([127, 0, 0, 1].into(), 30304);
    let config = NetworkConfig::builder(Arc::new(TestApi::default()), secret_key)
        .listener_addr(reth_p2p_socket)
        .discovery_addr(reth_disc_socket)
        .peer_config(peer_config)
        .build();

    let network = NetworkManager::new(config).await.unwrap();

    let handle = network.handle().clone();
    tokio::task::spawn(network);

    // make geth connect to us
    let our_peer_id = handle.peer_id();
    let our_enode = format!("enode://{}@{}", hex::encode(our_peer_id.0), reth_p2p_socket);

    provider.add_peer(our_enode).await.unwrap();

    let events = handle.event_listener();
    let mut event_stream = NetworkEventStream::new(events);

    // check for session to be opened
    let incoming_peer_id = event_stream.next_session_established().await.unwrap();
    assert_eq!(incoming_peer_id, geth_peer_id);
    // check to see that the session was closed
    let incoming_peer_id = event_stream.next_session_closed().await.unwrap();
    assert_eq!(incoming_peer_id, geth_peer_id);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_incoming_connect_with_single_geth() {
    reth_tracing::init_tracing();
    let secret_key = SecretKey::new(&mut rand::thread_rng());

    // instantiate geth and add ourselves as a peer
    let temp_dir = tempfile::tempdir().unwrap().into_path();
    let geth = Geth::new().data_dir(temp_dir).disable_discovery().spawn();
    let geth_endpoint = SocketAddr::new([127, 0, 0, 1].into(), geth.port());
    let provider = Provider::<Http>::try_from(format!("http://{geth_endpoint}")).unwrap();

    // get the peer id we should be expecting
    let geth_peer_id: PeerId =
        provider.node_info().await.unwrap().enr.public_key().encode_uncompressed().into();

    let reth_p2p_socket = SocketAddr::new([127, 0, 0, 1].into(), 30305);
    let reth_disc_socket = SocketAddr::new([127, 0, 0, 1].into(), 30306);
    let config = NetworkConfig::builder(Arc::new(TestApi::default()), secret_key)
        .listener_addr(reth_p2p_socket)
        .discovery_addr(reth_disc_socket)
        .build();

    let network = NetworkManager::new(config).await.unwrap();

    let handle = network.handle().clone();
    tokio::task::spawn(network);

    // make geth connect to us
    let our_peer_id = handle.peer_id();
    let our_enode = format!("enode://{}@{}", hex::encode(our_peer_id.0), reth_p2p_socket);

    provider.add_peer(our_enode).await.unwrap();

    let events = handle.event_listener();
    let mut event_stream = NetworkEventStream::new(events);

    // check for a sessionestablished event
    let incoming_peer_id = event_stream.next_session_established().await.unwrap();
    assert_eq!(incoming_peer_id, geth_peer_id);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_outgoing_connect_with_single_geth() {
    reth_tracing::init_tracing();
    let secret_key = SecretKey::new(&mut rand::thread_rng());

    let reth_p2p_socket = SocketAddr::new([127, 0, 0, 1].into(), 30307);
    let reth_disc_socket = SocketAddr::new([127, 0, 0, 1].into(), 30308);
    let config = NetworkConfig::builder(Arc::new(TestApi::default()), secret_key)
        .listener_addr(reth_p2p_socket)
        .discovery_addr(reth_disc_socket)
        .build();
    let network = NetworkManager::new(config).await.unwrap();

    let handle = network.handle().clone();
    tokio::task::spawn(network);

    // instantiate geth and add ourselves as a peer
    let temp_dir = tempfile::tempdir().unwrap().into_path();
    let geth = Geth::new().disable_discovery().data_dir(temp_dir).spawn();

    let geth_p2p_port = geth.p2p_port().unwrap();
    let geth_socket = SocketAddr::new([127, 0, 0, 1].into(), geth_p2p_port);
    let geth_endpoint = SocketAddr::new([127, 0, 0, 1].into(), geth.port()).to_string();

    let provider = Provider::<Http>::try_from(format!("http://{geth_endpoint}")).unwrap();

    // get the peer id we should be expecting
    let geth_peer_id: PeerId =
        provider.node_info().await.unwrap().enr.public_key().encode_uncompressed().into();

    // add geth as a peer then wait for a `SessionEstablished` event
    handle.add_peer(geth_peer_id, geth_socket);

    // create networkeventstream to get the next session established event easily
    let events = handle.event_listener();
    let mut event_stream = NetworkEventStream::new(events);

    // check for a sessionestablished event
    let incoming_peer_id = event_stream.next_session_established().await.unwrap();
    assert_eq!(incoming_peer_id, geth_peer_id);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_geth_disconnect() {
    reth_tracing::init_tracing();
    let secret_key = SecretKey::new(&mut rand::thread_rng());

    let reth_p2p_socket = SocketAddr::new([127, 0, 0, 1].into(), 30309);
    let reth_disc_socket = SocketAddr::new([127, 0, 0, 1].into(), 30310);
    let config = NetworkConfig::builder(Arc::new(TestApi::default()), secret_key)
        .listener_addr(reth_p2p_socket)
        .discovery_addr(reth_disc_socket)
        .build();
    let network = NetworkManager::new(config).await.unwrap();

    let handle = network.handle().clone();
    tokio::task::spawn(network);

    // instantiate geth and add ourselves as a peer
    let temp_dir = tempfile::tempdir().unwrap().into_path();
    let geth = Geth::new().disable_discovery().data_dir(temp_dir).spawn();

    let geth_p2p_port = geth.p2p_port().unwrap();
    let geth_socket = SocketAddr::new([127, 0, 0, 1].into(), geth_p2p_port);
    let geth_endpoint = SocketAddr::new([127, 0, 0, 1].into(), geth.port()).to_string();

    let provider = Provider::<Http>::try_from(format!("http://{geth_endpoint}")).unwrap();

    // get the peer id we should be expecting
    let geth_peer_id: PeerId =
        provider.node_info().await.unwrap().enr.public_key().encode_uncompressed().into();

    // add geth as a peer then wait for a `SessionEstablished` event
    handle.add_peer(geth_peer_id, geth_socket);

    // create networkeventstream to get the next session established event easily
    let mut events = handle.event_listener();

    if let Some(NetworkEvent::SessionEstablished { peer_id, .. }) = events.next().await {
        assert_eq!(peer_id, geth_peer_id);
    } else {
        panic!("Expected a session established event");
    }

    // remove geth as a peer deliberately
    handle.disconnect_peer(geth_peer_id);

    // wait for a disconnect from geth
    if let Some(NetworkEvent::SessionClosed { peer_id }) = events.next().await {
        assert_eq!(peer_id, geth_peer_id);
    } else {
        panic!("Expected a session closed event");
    }
}

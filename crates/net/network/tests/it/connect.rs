//! Connection tests

use enr::EnrPublicKey;
use super::testnet::Testnet;
use ethers_core::utils::Geth;
use ethers_providers::{Http, Provider, Middleware};
use futures::StreamExt;
use reth_discv4::{bootnodes::mainnet_nodes, Discv4Config};
use reth_network::{NetworkConfig, NetworkEvent, NetworkManager};
use reth_provider::test_utils::TestApi;
use reth_primitives::PeerId;
use secp256k1::SecretKey;
use std::{collections::HashSet, sync::Arc, net::SocketAddr};

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
async fn test_connect_with_single_geth() {
    reth_tracing::init_tracing();
    let secret_key = SecretKey::new(&mut rand::thread_rng());

    let config =
        NetworkConfig::builder(Arc::new(TestApi::default()), secret_key).build();
    let network = NetworkManager::new(config).await.unwrap();

    let handle = network.handle().clone();
    let mut events = handle.event_listener();
    tokio::task::spawn(network);

    // instantiate geth and add ourselves as a peer
    let geth = Geth::new().disable_discovery().spawn();
    let geth_p2p_port = geth.p2p_port().unwrap();
    let geth_socket = SocketAddr::from(([127, 0, 0, 1], geth_p2p_port));
    let geth_endpoint = geth_socket.to_string();
    println!("geth endpoint: {}", geth_endpoint);
    let provider = Provider::<Http>::try_from(format!("http://localhost:{}", geth_p2p_port)).unwrap();
    let peer_id: PeerId = provider.node_info().await.unwrap().enr.public_key().encode_uncompressed().into();

    handle.add_peer(peer_id, geth_socket);

    while let Some(ev) = events.next().await {
        dbg!(ev);
    }
}

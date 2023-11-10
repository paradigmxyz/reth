//! Session tests

use futures::StreamExt;
use reth_eth_wire::EthVersion;
use reth_network::{
    test_utils::{PeerConfig, Testnet},
    NetworkEvent, NetworkEvents,
};
use reth_network_api::{NetworkInfo, Peers};
use reth_provider::test_utils::NoopProvider;

#[tokio::test(flavor = "multi_thread")]
async fn test_session_established_with_highest_version() {
    reth_tracing::init_test_tracing();

    let net = Testnet::create(2).await;
    net.for_each(|peer| assert_eq!(0, peer.num_peers()));

    let mut handles = net.handles();
    let handle0 = handles.next().unwrap();
    let handle1 = handles.next().unwrap();
    drop(handles);

    let handle = net.spawn();

    let mut events = handle0.event_listener().take(2);
    handle0.add_peer(*handle1.peer_id(), handle1.local_addr());

    while let Some(event) = events.next().await {
        match event {
            NetworkEvent::PeerAdded(peer_id) => {
                assert_eq!(handle1.peer_id(), &peer_id);
            }
            NetworkEvent::SessionEstablished { peer_id, status, .. } => {
                assert_eq!(handle1.peer_id(), &peer_id);
                assert_eq!(status.version, EthVersion::Eth68 as u8);
            }
            ev => {
                panic!("unexpected event {ev:?}")
            }
        }
    }
    handle.terminate().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_established_with_different_capability() {
    reth_tracing::init_test_tracing();

    let mut net = Testnet::create(1).await;

    let p1 = PeerConfig::with_protocols(NoopProvider::default(), Some(EthVersion::Eth66.into()));
    net.add_peer_with_config(p1).await.unwrap();

    net.for_each(|peer| assert_eq!(0, peer.num_peers()));

    let mut handles = net.handles();
    let handle0 = handles.next().unwrap();
    let handle1 = handles.next().unwrap();
    drop(handles);

    let handle = net.spawn();

    let mut events = handle0.event_listener().take(2);
    handle0.add_peer(*handle1.peer_id(), handle1.local_addr());

    while let Some(event) = events.next().await {
        match event {
            NetworkEvent::PeerAdded(peer_id) => {
                assert_eq!(handle1.peer_id(), &peer_id);
            }
            NetworkEvent::SessionEstablished { peer_id, status, .. } => {
                assert_eq!(handle1.peer_id(), &peer_id);
                assert_eq!(status.version, EthVersion::Eth66 as u8);
            }
            ev => {
                panic!("unexpected event: {ev:?}")
            }
        }
    }

    handle.terminate().await;
}

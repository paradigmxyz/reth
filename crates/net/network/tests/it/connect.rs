//! Connection tests

use super::testnet::Testnet;
use futures::StreamExt;
use reth_network::NetworkEvent;
use std::collections::HashSet;

#[tokio::test(flavor = "multi_thread")]
async fn test_establish_connections() {
    let net = Testnet::create(3).await;

    net.for_each(|peer| assert_eq!(0, peer.num_peers()));

    let mut handles = net.handles();
    let handle0 = handles.next().unwrap();
    let handle1 = handles.next().unwrap();
    let handle2 = handles.next().unwrap();

    drop(handles);
    let handle = net.spawn();

    let listener0 = handle0.event_listener();

    handle0.add_peer(*handle1.peer_id(), handle1.local_addr());
    handle0.add_peer(*handle2.peer_id(), handle2.local_addr());

    let mut expected_connections = HashSet::from([*handle1.peer_id(), *handle2.peer_id()]);

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

    let net = handle.terminate().await;

    assert_eq!(net.peers()[0].num_peers(), 2);
    assert_eq!(net.peers()[1].num_peers(), 1);
    assert_eq!(net.peers()[2].num_peers(), 1);
}

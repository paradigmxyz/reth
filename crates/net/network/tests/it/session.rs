//! Session tests

use futures::StreamExt;
use reth_eth_wire::EthVersion;
use reth_network::{
    test_utils::{NetworkEventStream, PeerConfig, Testnet},
    NetworkEvent, NetworkEventListenerProvider,
};
use reth_network_api::{
    events::{PeerEvent, SessionInfo},
    NetworkInfo, Peers,
};
use reth_storage_api::noop::NoopProvider;

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
            NetworkEvent::Peer(PeerEvent::PeerAdded(peer_id)) => {
                assert_eq!(handle1.peer_id(), &peer_id);
            }
            NetworkEvent::ActivePeerSession { info, .. } => {
                let SessionInfo { peer_id, status, .. } = info;
                assert_eq!(handle1.peer_id(), &peer_id);
                assert_eq!(status.version, EthVersion::LATEST);
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
            NetworkEvent::Peer(PeerEvent::PeerAdded(peer_id)) => {
                assert_eq!(handle1.peer_id(), &peer_id);
            }
            NetworkEvent::ActivePeerSession { info, .. } => {
                let SessionInfo { peer_id, status, .. } = info;
                assert_eq!(handle1.peer_id(), &peer_id);
                assert_eq!(status.version, EthVersion::Eth66);
            }
            ev => {
                panic!("unexpected event: {ev:?}")
            }
        }
    }

    handle.terminate().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_capability_version_mismatch() {
    reth_tracing::init_test_tracing();

    let mut net = Testnet::create(0).await;

    let p0 = PeerConfig::with_protocols(NoopProvider::default(), Some(EthVersion::Eth66.into()));
    net.add_peer_with_config(p0).await.unwrap();

    let p1 = PeerConfig::with_protocols(NoopProvider::default(), Some(EthVersion::Eth67.into()));
    net.add_peer_with_config(p1).await.unwrap();

    net.for_each(|peer| assert_eq!(0, peer.num_peers()));

    let mut handles = net.handles();
    let handle0 = handles.next().unwrap();
    let handle1 = handles.next().unwrap();
    drop(handles);

    let handle = net.spawn();

    let events = handle0.event_listener();

    let mut event_stream = NetworkEventStream::new(events);

    handle0.add_peer(*handle1.peer_id(), handle1.local_addr());

    let added_peer_id = event_stream.peer_added().await.unwrap();
    assert_eq!(added_peer_id, *handle1.peer_id());

    // peer with mismatched capability version should fail to connect and be removed.
    let removed_peer_id = event_stream.peer_removed().await.unwrap();
    assert_eq!(removed_peer_id, *handle1.peer_id());

    handle.terminate().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth69_peers_can_connect() {
    reth_tracing::init_test_tracing();

    let mut net = Testnet::create(0).await;

    // Create two peers that only support ETH69
    let p0 = PeerConfig::with_protocols(NoopProvider::default(), Some(EthVersion::Eth69.into()));
    net.add_peer_with_config(p0).await.unwrap();

    let p1 = PeerConfig::with_protocols(NoopProvider::default(), Some(EthVersion::Eth69.into()));
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
            NetworkEvent::Peer(PeerEvent::PeerAdded(peer_id)) => {
                assert_eq!(handle1.peer_id(), &peer_id);
            }
            NetworkEvent::ActivePeerSession { info, .. } => {
                let SessionInfo { peer_id, status, .. } = info;
                assert_eq!(handle1.peer_id(), &peer_id);
                // Both peers support only ETH69, so they should connect with ETH69
                assert_eq!(status.version, EthVersion::Eth69);
            }
            ev => {
                panic!("unexpected event: {ev:?}")
            }
        }
    }

    handle.terminate().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_peers_negotiate_highest_version_eth69() {
    reth_tracing::init_test_tracing();

    let mut net = Testnet::create(0).await;

    // Create one peer with multiple ETH versions including ETH69
    let p0 = PeerConfig::with_protocols(
        NoopProvider::default(),
        vec![
            EthVersion::Eth69.into(),
            EthVersion::Eth68.into(),
            EthVersion::Eth67.into(),
            EthVersion::Eth66.into(),
        ],
    );
    net.add_peer_with_config(p0).await.unwrap();

    // Create another peer with multiple ETH versions including ETH69
    let p1 = PeerConfig::with_protocols(
        NoopProvider::default(),
        vec![EthVersion::Eth69.into(), EthVersion::Eth68.into(), EthVersion::Eth67.into()],
    );
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
            NetworkEvent::Peer(PeerEvent::PeerAdded(peer_id)) => {
                assert_eq!(handle1.peer_id(), &peer_id);
            }
            NetworkEvent::ActivePeerSession { info, .. } => {
                let SessionInfo { peer_id, status, .. } = info;
                assert_eq!(handle1.peer_id(), &peer_id);
                // Both peers support ETH69, so they should negotiate to the highest version: ETH69
                assert_eq!(status.version, EthVersion::Eth69);
            }
            ev => {
                panic!("unexpected event: {ev:?}")
            }
        }
    }

    handle.terminate().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth69_vs_eth68_incompatible() {
    reth_tracing::init_test_tracing();

    let mut net = Testnet::create(0).await;

    // Create one peer that only supports ETH69
    let p0 = PeerConfig::with_protocols(NoopProvider::default(), Some(EthVersion::Eth69.into()));
    net.add_peer_with_config(p0).await.unwrap();

    // Create another peer that only supports ETH68
    let p1 = PeerConfig::with_protocols(NoopProvider::default(), Some(EthVersion::Eth68.into()));
    net.add_peer_with_config(p1).await.unwrap();

    net.for_each(|peer| assert_eq!(0, peer.num_peers()));

    let mut handles = net.handles();
    let handle0 = handles.next().unwrap();
    let handle1 = handles.next().unwrap();
    drop(handles);

    let handle = net.spawn();

    let events = handle0.event_listener();
    let mut event_stream = NetworkEventStream::new(events);

    handle0.add_peer(*handle1.peer_id(), handle1.local_addr());

    let added_peer_id = event_stream.peer_added().await.unwrap();
    assert_eq!(added_peer_id, *handle1.peer_id());

    // Peers with no shared ETH version should fail to connect and be removed.
    let removed_peer_id = event_stream.peer_removed().await.unwrap();
    assert_eq!(removed_peer_id, *handle1.peer_id());

    handle.terminate().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth69_mixed_version_negotiation() {
    reth_tracing::init_test_tracing();

    let mut net = Testnet::create(0).await;

    // Create one peer that supports ETH69 + ETH68
    let p0 = PeerConfig::with_protocols(
        NoopProvider::default(),
        vec![EthVersion::Eth69.into(), EthVersion::Eth68.into()],
    );
    net.add_peer_with_config(p0).await.unwrap();

    // Create another peer that only supports ETH68
    let p1 = PeerConfig::with_protocols(NoopProvider::default(), Some(EthVersion::Eth68.into()));
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
            NetworkEvent::Peer(PeerEvent::PeerAdded(peer_id)) => {
                assert_eq!(handle1.peer_id(), &peer_id);
            }
            NetworkEvent::ActivePeerSession { info, .. } => {
                let SessionInfo { peer_id, status, .. } = info;
                assert_eq!(handle1.peer_id(), &peer_id);
                // Should negotiate to ETH68 (highest common version)
                assert_eq!(status.version, EthVersion::Eth68);
            }
            ev => {
                panic!("unexpected event: {ev:?}")
            }
        }
    }

    handle.terminate().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_multiple_peers_different_eth_versions() {
    reth_tracing::init_test_tracing();

    let mut net = Testnet::create(0).await;

    // Create a peer that supports all versions (ETH66-ETH69)
    let p0 = PeerConfig::with_protocols(
        NoopProvider::default(),
        vec![
            EthVersion::Eth69.into(),
            EthVersion::Eth68.into(),
            EthVersion::Eth67.into(),
            EthVersion::Eth66.into(),
        ],
    );
    net.add_peer_with_config(p0).await.unwrap();

    // Create a peer that only supports newer versions (ETH68-ETH69)
    let p1 = PeerConfig::with_protocols(
        NoopProvider::default(),
        vec![EthVersion::Eth69.into(), EthVersion::Eth68.into()],
    );
    net.add_peer_with_config(p1).await.unwrap();

    // Create a peer that only supports older versions (ETH66-ETH67)
    let p2 = PeerConfig::with_protocols(
        NoopProvider::default(),
        vec![EthVersion::Eth67.into(), EthVersion::Eth66.into()],
    );
    net.add_peer_with_config(p2).await.unwrap();

    net.for_each(|peer| assert_eq!(0, peer.num_peers()));

    let mut handles = net.handles();
    let handle0 = handles.next().unwrap(); // All versions peer
    let handle1 = handles.next().unwrap(); // Newer versions peer
    let handle2 = handles.next().unwrap(); // Older versions peer
    drop(handles);

    let handle = net.spawn();

    let events = handle0.event_listener();
    let mut event_stream = NetworkEventStream::new(events);

    // Connect peer0 (all versions) to peer1 (newer versions) - should negotiate ETH69
    handle0.add_peer(*handle1.peer_id(), handle1.local_addr());

    let added_peer_id = event_stream.peer_added().await.unwrap();
    assert_eq!(added_peer_id, *handle1.peer_id());

    let established_peer_id = event_stream.next_session_established().await.unwrap();
    assert_eq!(established_peer_id, *handle1.peer_id());

    // Connect peer0 (all versions) to peer2 (older versions) - should negotiate ETH67
    handle0.add_peer(*handle2.peer_id(), handle2.local_addr());

    let added_peer_id = event_stream.peer_added().await.unwrap();
    assert_eq!(added_peer_id, *handle2.peer_id());

    let established_peer_id = event_stream.next_session_established().await.unwrap();
    assert_eq!(established_peer_id, *handle2.peer_id());

    // Both connections should be established successfully

    handle.terminate().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth69_capability_negotiation_fallback() {
    reth_tracing::init_test_tracing();

    let mut net = Testnet::create(0).await;

    // Create a peer that prefers ETH69 but supports fallback to ETH67
    let p0 = PeerConfig::with_protocols(
        NoopProvider::default(),
        vec![EthVersion::Eth69.into(), EthVersion::Eth67.into()],
    );
    net.add_peer_with_config(p0).await.unwrap();

    // Create a peer that skips ETH68 and only supports ETH67/ETH66
    let p1 = PeerConfig::with_protocols(
        NoopProvider::default(),
        vec![EthVersion::Eth67.into(), EthVersion::Eth66.into()],
    );
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
            NetworkEvent::Peer(PeerEvent::PeerAdded(peer_id)) => {
                assert_eq!(handle1.peer_id(), &peer_id);
            }
            NetworkEvent::ActivePeerSession { info, .. } => {
                let SessionInfo { peer_id, status, .. } = info;
                assert_eq!(handle1.peer_id(), &peer_id);
                // Should fallback to ETH67 (skipping ETH68 which neither supports)
                assert_eq!(status.version, EthVersion::Eth67);
            }
            ev => {
                panic!("unexpected event: {ev:?}")
            }
        }
    }

    handle.terminate().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn test_overlapping_version_sets_negotiation() {
    reth_tracing::init_test_tracing();

    let mut net = Testnet::create(0).await;

    // Peer 0: supports ETH69, ETH67, ETH66 (skips ETH68)
    let p0 = PeerConfig::with_protocols(
        NoopProvider::default(),
        vec![EthVersion::Eth69.into(), EthVersion::Eth67.into(), EthVersion::Eth66.into()],
    );
    net.add_peer_with_config(p0).await.unwrap();

    // Peer 1: supports ETH68, ETH67, ETH66 (skips ETH69)
    let p1 = PeerConfig::with_protocols(
        NoopProvider::default(),
        vec![EthVersion::Eth68.into(), EthVersion::Eth67.into(), EthVersion::Eth66.into()],
    );
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
            NetworkEvent::Peer(PeerEvent::PeerAdded(peer_id)) => {
                assert_eq!(handle1.peer_id(), &peer_id);
            }
            NetworkEvent::ActivePeerSession { info, .. } => {
                let SessionInfo { peer_id, status, .. } = info;
                assert_eq!(handle1.peer_id(), &peer_id);
                // Should negotiate to ETH67 (highest common version between ETH69,67,66 and
                // ETH68,67,66)
                assert_eq!(status.version, EthVersion::Eth67);
            }
            ev => {
                panic!("unexpected event: {ev:?}")
            }
        }
    }

    handle.terminate().await;
}

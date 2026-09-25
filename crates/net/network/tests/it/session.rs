//! Session tests

use futures::StreamExt;
use reth_eth_wire::EthVersion;
use reth_network::{
    test_utils::{PeerConfig, PeerHandle, Testnet},
    NetworkEvent,
};
use reth_network_api::events::PeerEvent;

#[tokio::test(flavor = "multi_thread")]
async fn test_session_established_with_highest_version() {
    let version = negotiated_version(PeerConfig::default(), PeerConfig::default()).await;
    assert_eq!(version, Some(EthVersion::LATEST));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_session_established_with_different_capability() {
    let version = negotiated_version(
        PeerConfig::default(),
        PeerConfig::default().with_protocols([EthVersion::Eth66]),
    )
    .await;
    assert_eq!(version, Some(EthVersion::Eth66));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_capability_version_mismatch() {
    let version = negotiated_version(
        PeerConfig::default().with_protocols([EthVersion::Eth66]),
        PeerConfig::default().with_protocols([EthVersion::Eth67]),
    )
    .await;
    // peer with mismatched capability version should fail to connect and be removed.
    assert_eq!(version, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth69_peers_can_connect() {
    let version = negotiated_version(
        PeerConfig::default().with_protocols([EthVersion::Eth69]),
        PeerConfig::default().with_protocols([EthVersion::Eth69]),
    )
    .await;
    assert_eq!(version, Some(EthVersion::Eth69));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_peers_negotiate_highest_version_eth69() {
    let version = negotiated_version(
        PeerConfig::default().with_protocols([
            EthVersion::Eth69,
            EthVersion::Eth68,
            EthVersion::Eth67,
            EthVersion::Eth66,
        ]),
        PeerConfig::default().with_protocols([
            EthVersion::Eth69,
            EthVersion::Eth68,
            EthVersion::Eth67,
        ]),
    )
    .await;
    assert_eq!(version, Some(EthVersion::Eth69));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth69_vs_eth68_incompatible() {
    let version = negotiated_version(
        PeerConfig::default().with_protocols([EthVersion::Eth69]),
        PeerConfig::default().with_protocols([EthVersion::Eth68]),
    )
    .await;
    // Peers with no shared ETH version should fail to connect and be removed.
    assert_eq!(version, None);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth69_mixed_version_negotiation() {
    let version = negotiated_version(
        PeerConfig::default().with_protocols([EthVersion::Eth69, EthVersion::Eth68]),
        PeerConfig::default().with_protocols([EthVersion::Eth68]),
    )
    .await;
    assert_eq!(version, Some(EthVersion::Eth68));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_multiple_peers_different_eth_versions() {
    reth_tracing::init_test_tracing();

    let net = Testnet::from_configs([
        // supports all versions
        PeerConfig::default().with_protocols([
            EthVersion::Eth69,
            EthVersion::Eth68,
            EthVersion::Eth67,
            EthVersion::Eth66,
        ]),
        // only supports newer versions
        PeerConfig::default().with_protocols([EthVersion::Eth69, EthVersion::Eth68]),
        // only supports older versions
        PeerConfig::default().with_protocols([EthVersion::Eth67, EthVersion::Eth66]),
    ])
    .await
    .spawn();
    let [all, newer, older] = net.peers_array();

    assert_eq!(connect(all, newer).await, Some(EthVersion::Eth69));
    assert_eq!(connect(all, older).await, Some(EthVersion::Eth67));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_eth69_capability_negotiation_fallback() {
    let version = negotiated_version(
        PeerConfig::default().with_protocols([EthVersion::Eth69, EthVersion::Eth67]),
        PeerConfig::default().with_protocols([EthVersion::Eth67, EthVersion::Eth66]),
    )
    .await;
    // Should fallback to ETH67 (skipping ETH68 which neither supports)
    assert_eq!(version, Some(EthVersion::Eth67));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_overlapping_version_sets_negotiation() {
    let version = negotiated_version(
        PeerConfig::default().with_protocols([
            EthVersion::Eth69,
            EthVersion::Eth67,
            EthVersion::Eth66,
        ]),
        PeerConfig::default().with_protocols([
            EthVersion::Eth68,
            EthVersion::Eth67,
            EthVersion::Eth66,
        ]),
    )
    .await;
    // Should negotiate to ETH67 (highest common version between ETH69,67,66 and ETH68,67,66)
    assert_eq!(version, Some(EthVersion::Eth67));
}

/// Launches a peer from each config, connects the first to the second and returns the negotiated
/// eth version.
async fn negotiated_version(a: PeerConfig, b: PeerConfig) -> Option<EthVersion> {
    reth_tracing::init_test_tracing();

    let net = Testnet::from_configs([a, b]).await.spawn();
    let [a, b] = net.peers_array();
    connect(a, b).await
}

/// Connects `peer` to `other` and returns the negotiated eth version, or `None` if `other` was
/// removed because no session could be established.
async fn connect<Pool>(peer: &PeerHandle<Pool>, other: &PeerHandle<Pool>) -> Option<EthVersion> {
    let mut events = peer.event_listener();
    peer.add_peer(other);

    match events.next().await {
        Some(NetworkEvent::Peer(PeerEvent::PeerAdded(peer_id))) => {
            assert_eq!(peer_id, *other.peer_id())
        }
        event => panic!("unexpected event {event:?}"),
    }

    match events.next().await {
        Some(NetworkEvent::ActivePeerSession { info, .. }) if info.peer_id == *other.peer_id() => {
            Some(info.status.version)
        }
        Some(NetworkEvent::Peer(PeerEvent::PeerRemoved(peer_id)))
            if peer_id == *other.peer_id() =>
        {
            None
        }
        event => panic!("unexpected event {event:?}"),
    }
}

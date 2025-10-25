use alloy_rlp::{Decodable, Encodable};
use example_bsc_p2p::upgrade_status::{UpgradeStatus, UpgradeStatusExtension};

#[test]
fn test_upgrade_status_extension_constructors() {
    // Test new constructor
    let extension = UpgradeStatusExtension::new(true);
    assert!(extension.disable_peer_tx_broadcast);

    let extension = UpgradeStatusExtension::new(false);
    assert!(!extension.disable_peer_tx_broadcast);

    // Test allow_broadcast constructor
    let extension = UpgradeStatusExtension::allow_broadcast();
    assert!(!extension.disable_peer_tx_broadcast);

    // Test disable_broadcast constructor
    let extension = UpgradeStatusExtension::disable_broadcast();
    assert!(extension.disable_peer_tx_broadcast);
}

#[test]
fn test_upgrade_status_constructors() {
    // Test new constructor
    let extension = UpgradeStatusExtension::new(true);
    let status = UpgradeStatus::new(extension);
    assert!(status.extension.disable_peer_tx_broadcast);

    // Test allow_broadcast constructor
    let status = UpgradeStatus::allow_broadcast();
    assert!(!status.extension.disable_peer_tx_broadcast);

    // Test disable_broadcast constructor
    let status = UpgradeStatus::disable_broadcast();
    assert!(status.extension.disable_peer_tx_broadcast);
}

#[test]
fn test_upgrade_status_rlpx_encoding() {
    // Test that into_rlpx() produces valid bytes
    let status = UpgradeStatus::allow_broadcast();
    let encoded = status.into_rlpx();
    assert!(!encoded.is_empty());

    let status = UpgradeStatus::disable_broadcast();
    let encoded = status.into_rlpx();
    assert!(!encoded.is_empty());
}

#[test]
fn test_upgrade_status_extension_encoding_decoding() {
    // Test encoding and decoding for allow_broadcast
    let original = UpgradeStatusExtension::allow_broadcast();
    let mut encoded = Vec::new();
    original.encode(&mut encoded);
    let decoded = UpgradeStatusExtension::decode(&mut encoded.as_slice()).unwrap();
    assert_eq!(original, decoded);

    // Test encoding and decoding for disable_broadcast
    let original = UpgradeStatusExtension::disable_broadcast();
    let mut encoded = Vec::new();
    original.encode(&mut encoded);
    let decoded = UpgradeStatusExtension::decode(&mut encoded.as_slice()).unwrap();
    assert_eq!(original, decoded);
}

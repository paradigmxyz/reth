use reth_discv4::{Discv4Config, DEFAULT_DISCOVERY_PORT};
use reth_network::{
    error::{NetworkError, ServiceKind},
    Discovery, NetworkConfigBuilder, NetworkManager,
};
use reth_provider::test_utils::NoopProvider;
use secp256k1::SecretKey;
use std::{
    io,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
};

fn is_addr_in_use_kind(err: NetworkError, kind: ServiceKind) -> bool {
    match err {
        NetworkError::AddressAlreadyInUse { kind: k, error } => {
            k == kind && error.kind() == io::ErrorKind::AddrInUse
        }
        _ => false,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_listener_addr_in_use() {
    let secret_key = SecretKey::new(&mut rand::thread_rng());
    let config = NetworkConfigBuilder::new(secret_key).build(NoopProvider::default());
    let _network = NetworkManager::new(config).await.unwrap();
    let config = NetworkConfigBuilder::new(secret_key).build(NoopProvider::default());
    let addr = config.discovery_addr;
    let result = NetworkManager::new(config).await;
    assert!(is_addr_in_use_kind(result.err().unwrap(), ServiceKind::Listener(addr)));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_discovery_addr_in_use() {
    let secret_key = SecretKey::new(&mut rand::thread_rng());
    let disc_config = Discv4Config::default();
    let addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, DEFAULT_DISCOVERY_PORT));
    let _discovery = Discovery::new(addr, secret_key, Some(disc_config), None).await.unwrap();
    let disc_config = Discv4Config::default();
    let result = Discovery::new(addr, secret_key, Some(disc_config), None).await;
    assert!(is_addr_in_use_kind(result.err().unwrap(), ServiceKind::Discovery(addr)));
}

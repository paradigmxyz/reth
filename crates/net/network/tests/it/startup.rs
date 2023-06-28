use reth_discv4::Discv4Config;
use reth_network::{
    error::{NetworkError, ServiceKind},
    Discovery, NetworkConfigBuilder, NetworkManager,
};
use reth_network_api::NetworkInfo;
use reth_provider::test_utils::NoopProvider;
use secp256k1::SecretKey;
use std::{
    io,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
};
use tokio::net::TcpListener;

fn is_addr_in_use_kind(err: &NetworkError, kind: ServiceKind) -> bool {
    match err {
        NetworkError::AddressAlreadyInUse { kind: k, error } => {
            *k == kind && error.kind() == io::ErrorKind::AddrInUse
        }
        _ => false,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_is_default_syncing() {
    let secret_key = SecretKey::new(&mut rand::thread_rng());
    let config = NetworkConfigBuilder::new(secret_key)
        .disable_discovery()
        .listener_port(0)
        .build(NoopProvider::default());
    let network = NetworkManager::new(config).await.unwrap();
    assert!(!network.handle().is_syncing());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_listener_addr_in_use() {
    let secret_key = SecretKey::new(&mut rand::thread_rng());
    let config =
        NetworkConfigBuilder::new(secret_key).listener_port(0).build(NoopProvider::default());
    let network = NetworkManager::new(config).await.unwrap();
    let listener_port = network.local_addr().port();
    let config = NetworkConfigBuilder::new(secret_key)
        .listener_port(listener_port)
        .build(NoopProvider::default());
    let addr = config.listener_addr;
    let result = NetworkManager::new(config).await;
    let err = result.err().unwrap();
    assert!(is_addr_in_use_kind(&err, ServiceKind::Listener(addr)), "{:?}", err);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_discovery_addr_in_use() {
    let secret_key = SecretKey::new(&mut rand::thread_rng());
    let disc_config = Discv4Config::default();
    let addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0));
    let any_port_listener = TcpListener::bind(addr).await.unwrap();
    let port = any_port_listener.local_addr().unwrap().port();
    let addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port));
    let _discovery = Discovery::new(addr, secret_key, Some(disc_config), None).await.unwrap();
    let disc_config = Discv4Config::default();
    let result = Discovery::new(addr, secret_key, Some(disc_config), None).await;
    assert!(is_addr_in_use_kind(&result.err().unwrap(), ServiceKind::Discovery(addr)));
}

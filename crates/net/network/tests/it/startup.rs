use std::{
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4},
};

use reth_chainspec::MAINNET;
use reth_discv4::{Discv4Config, NatResolver, DEFAULT_DISCOVERY_ADDR, DEFAULT_DISCOVERY_PORT};
use reth_network::{
    error::{NetworkError, ServiceKind},
    Discovery, NetworkConfigBuilder, NetworkManager,
};
use reth_network_api::{NetworkInfo, PeersInfo};
use reth_storage_api::noop::NoopProvider;
use secp256k1::SecretKey;
use tokio::net::TcpListener;

fn is_addr_in_use_kind(err: &NetworkError, kind: ServiceKind) -> bool {
    match err {
        NetworkError::AddressAlreadyInUse { kind: k, error } => {
            *k == kind && error.kind() == io::ErrorKind::AddrInUse
        }
        NetworkError::Discv5Error(reth_discv5::Error::Discv5Error(discv5::Error::Io(err))) => {
            err.kind() == io::ErrorKind::AddrInUse
        }
        _ => false,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_is_default_syncing() {
    let secret_key = SecretKey::new(&mut rand_08::thread_rng());
    let config = NetworkConfigBuilder::eth(secret_key)
        .disable_discovery()
        .listener_port(0)
        .build(NoopProvider::default());
    let network = NetworkManager::new(config).await.unwrap();
    assert!(!network.handle().is_syncing());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_listener_addr_in_use() {
    let secret_key = SecretKey::new(&mut rand_08::thread_rng());
    let config = NetworkConfigBuilder::eth(secret_key)
        .disable_discovery()
        .listener_port(0)
        .build(NoopProvider::default());
    let network = NetworkManager::new(config).await.unwrap();
    let listener_port = network.local_addr().port();
    let config = NetworkConfigBuilder::eth(secret_key)
        .listener_port(listener_port)
        .disable_discovery()
        .build(NoopProvider::default());
    let addr = config.listener_addr;
    let result = NetworkManager::new(config).await;
    let err = result.err().unwrap();
    assert!(is_addr_in_use_kind(&err, ServiceKind::Listener(addr)), "{err:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_discovery_addr_in_use() {
    let secret_key = SecretKey::new(&mut rand_08::thread_rng());
    let disc_config = Discv4Config::default();
    let addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0));
    let any_port_listener = TcpListener::bind(addr).await.unwrap();
    let port = any_port_listener.local_addr().unwrap().port();
    let addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port));
    let _discovery =
        Discovery::new(addr, addr, secret_key, Some(disc_config), None, None).await.unwrap();
    let disc_config = Discv4Config::default();
    let result = Discovery::new(addr, addr, secret_key, Some(disc_config), None, None).await;
    assert!(is_addr_in_use_kind(&result.err().unwrap(), ServiceKind::Discovery(addr)));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_discv5_and_discv4_same_socket_fails() {
    let secret_key = SecretKey::new(&mut rand_08::thread_rng());
    let config = NetworkConfigBuilder::eth(secret_key)
        .listener_port(DEFAULT_DISCOVERY_PORT)
        .discovery_v5(
            reth_discv5::Config::builder((DEFAULT_DISCOVERY_ADDR, DEFAULT_DISCOVERY_PORT).into())
                .discv5_config(
                    discv5::ConfigBuilder::new(discv5::ListenConfig::from_ip(
                        DEFAULT_DISCOVERY_ADDR,
                        DEFAULT_DISCOVERY_PORT,
                    ))
                    .build(),
                ),
        )
        .disable_dns_discovery()
        .build(NoopProvider::default());
    let addr = config.listener_addr;
    let result = NetworkManager::new(config).await;
    let err = result.err().unwrap();

    assert!(is_addr_in_use_kind(&err, ServiceKind::Listener(addr)), "{err:?}")
}

#[tokio::test(flavor = "multi_thread")]
async fn test_discv5_and_rlpx_same_socket_ok_without_discv4() {
    let secret_key = SecretKey::new(&mut rand_08::thread_rng());
    let config = NetworkConfigBuilder::eth(secret_key)
        .listener_port(DEFAULT_DISCOVERY_PORT)
        .disable_discv4_discovery()
        .discovery_v5(
            reth_discv5::Config::builder((DEFAULT_DISCOVERY_ADDR, DEFAULT_DISCOVERY_PORT).into())
                .discv5_config(
                    discv5::ConfigBuilder::new(discv5::ListenConfig::from_ip(
                        DEFAULT_DISCOVERY_ADDR,
                        DEFAULT_DISCOVERY_PORT,
                    ))
                    .build(),
                ),
        )
        .disable_dns_discovery()
        .build(NoopProvider::default());
    let _network = NetworkManager::new(config).await.expect("should build");
}

// <https://github.com/paradigmxyz/reth/issues/8851>
#[tokio::test(flavor = "multi_thread")]
async fn test_tcp_port_node_record_no_discovery() {
    let secret_key = SecretKey::new(&mut rand_08::thread_rng());
    let config = NetworkConfigBuilder::eth(secret_key)
        .listener_port(0)
        .disable_discovery()
        .build_with_noop_provider(MAINNET.clone());
    let network = NetworkManager::new(config).await.unwrap();

    let local_addr = network.local_addr();
    // ensure we retrieved the port the OS chose
    assert_ne!(local_addr.port(), 0);

    let record = network.handle().local_node_record();
    assert_eq!(record.tcp_port, local_addr.port());
}

// <https://github.com/paradigmxyz/reth/issues/8851>
#[tokio::test(flavor = "multi_thread")]
async fn test_tcp_port_node_record_discovery() {
    let secret_key = SecretKey::new(&mut rand_08::thread_rng());
    let config = NetworkConfigBuilder::eth(secret_key)
        .listener_port(0)
        .discovery_port(0)
        .disable_dns_discovery()
        .build_with_noop_provider(MAINNET.clone());
    let network = NetworkManager::new(config).await.unwrap();

    let local_addr = network.local_addr();
    // ensure we retrieved the port the OS chose
    assert_ne!(local_addr.port(), 0);

    let record = network.handle().local_node_record();
    assert_eq!(record.tcp_port, local_addr.port());
    assert_ne!(record.udp_port, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_node_record_address_with_nat() {
    let secret_key = SecretKey::new(&mut rand_08::thread_rng());
    let config = NetworkConfigBuilder::eth(secret_key)
        .add_nat(Some(NatResolver::ExternalIp("10.1.1.1".parse().unwrap())))
        .disable_discv4_discovery()
        .disable_dns_discovery()
        .listener_port(0)
        .build_with_noop_provider(MAINNET.clone());

    let network = NetworkManager::new(config).await.unwrap();
    let record = network.handle().local_node_record();

    assert_eq!(record.address, IpAddr::V4(Ipv4Addr::new(10, 1, 1, 1)));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_node_record_address_with_nat_disable_discovery() {
    let secret_key = SecretKey::new(&mut rand_08::thread_rng());
    let config = NetworkConfigBuilder::eth(secret_key)
        .add_nat(Some(NatResolver::ExternalIp("10.1.1.1".parse().unwrap())))
        .disable_discovery()
        .disable_nat()
        .listener_port(0)
        .build_with_noop_provider(MAINNET.clone());

    let network = NetworkManager::new(config).await.unwrap();
    let record = network.handle().local_node_record();

    assert_eq!(record.address, IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
}

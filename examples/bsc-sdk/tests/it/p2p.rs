use example_bsc_sdk::{
    chainspec::bsc::{bsc_mainnet, head},
    node::network::{boot_nodes, handshake::BscHandshake},
};
use reth_chainspec::NamedChain;
use reth_discv4::Discv4Config;
use reth_network::{
    EthNetworkPrimitives, NetworkConfig, NetworkEvent, NetworkEventListenerProvider,
    NetworkManager, PeersInfo,
};
use reth_provider::noop::NoopProvider;
use secp256k1::{rand, SecretKey};
use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use tokio::{task, time::timeout};
use tokio_stream::StreamExt;

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn can_connect() {
    reth_tracing::init_test_tracing();
    let local_addr = SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 0);
    let secret_key = SecretKey::new(&mut rand::thread_rng());

    let client = NoopProvider::eth(Arc::new(bsc_mainnet()));

    let mut discv4 = Discv4Config::builder();
    discv4.add_boot_nodes(boot_nodes()).lookup_interval(Duration::from_millis(500));

    let net_cfg = NetworkConfig::<_, EthNetworkPrimitives>::builder(secret_key)
        .boot_nodes(boot_nodes())
        .set_head(head())
        .with_pow()
        .listener_addr(local_addr)
        .discovery(discv4)
        .eth_rlpx_handshake(Arc::new(BscHandshake::default()))
        .build(client.clone());

    let (handle, network, _, requests) = NetworkManager::new(net_cfg)
        .await
        .unwrap()
        .into_builder()
        .request_handler(client)
        .split_with_handle();
    let mut events = handle.event_listener();

    tokio::task::spawn(async move {
        tokio::join!(network, requests);
    });

    let result = timeout(Duration::from_secs(10), async {
        while let Some(evt) = events.next().await {
            if let NetworkEvent::ActivePeerSession { info, .. } = evt {
                assert_eq!(
                    info.status.chain.to_string(),
                    NamedChain::BinanceSmartChain.to_string()
                );
                return Ok(());
            }
        }
        Err("Expected event not received")
    })
    .await;

    assert!(result.is_ok(), "Test timed out without receiving the expected event");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn test_can_connect() {
    reth_tracing::init_test_tracing();
    let secret_key = SecretKey::new(&mut rand::thread_rng());
    let mut discv4 = Discv4Config::builder();
    discv4.add_boot_nodes(boot_nodes()).lookup_interval(Duration::from_millis(500));

    let client = NoopProvider::eth(Arc::new(bsc_mainnet()));

    let config = reth_network::NetworkConfigBuilder::eth(secret_key)
        .set_head(head())
        .with_pow()
        .eth_rlpx_handshake(Arc::new(BscHandshake::default()))
        .discovery(discv4)
        .build(client.clone());

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

use example_bsc_p2p::{
    chainspec::{boot_nodes, bsc_chain_spec, head},
    handshake::BscHandshake,
};
use reth_chainspec::NamedChain;
use reth_discv4::Discv4ConfigBuilder;
use reth_network::{
    EthNetworkPrimitives, NetworkConfig, NetworkEvent, NetworkEventListenerProvider, NetworkManager,
};
use reth_provider::noop::NoopProvider;
use secp256k1::{rand, SecretKey};
use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use tokio::time::timeout;
use tokio_stream::StreamExt;

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn can_connect() {
    reth_tracing::init_test_tracing();
    let local_addr = SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 30303);

    let secret_key = SecretKey::new(&mut rand::thread_rng());

    let net_cfg = NetworkConfig::<_, EthNetworkPrimitives>::builder(secret_key)
        .boot_nodes(boot_nodes())
        .set_head(head())
        .with_pow()
        .listener_addr(local_addr)
        .eth_rlpx_handshake(Arc::new(BscHandshake::default()))
        .build(NoopProvider::eth(bsc_chain_spec()));

    let net_cfg = net_cfg.set_discovery_v4(
        Discv4ConfigBuilder::default()
            .add_boot_nodes(boot_nodes())
            .lookup_interval(Duration::from_millis(500))
            .build(),
    );
    let net_manager = NetworkManager::<EthNetworkPrimitives>::new(net_cfg).await.unwrap();

    let net_handle = net_manager.handle().clone();
    let mut events = net_handle.event_listener();

    tokio::spawn(net_manager);

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

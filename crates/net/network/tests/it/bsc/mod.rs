use chainspec::{boot_nodes, bsc_chain_spec, head};
use futures::StreamExt;
use network::BscNetworkProtocol;
use reth_discv4::Discv4ConfigBuilder;
use reth_network::{
    EthNetworkPrimitives, NetworkConfig, NetworkEvent, NetworkEventListenerProvider,
    NetworkManager, PeersInfo,
};
use reth_network_api::events::{PeerEvent, SessionInfo};
use reth_provider::noop::NoopProvider;
use reth_tracing::{
    tracing_subscriber::filter::LevelFilter, LayerInfo, LogFormat, RethTracer, Tracer,
};
use secp256k1::{rand, SecretKey};
use std::{
    net::{Ipv4Addr, SocketAddr},
    time::Duration,
};
use tracing::info;

mod chainspec;
mod network;

#[tokio::test(flavor = "multi_thread")]
async fn test_bsc_network() {
    let _ = RethTracer::new()
        .with_stdout(LayerInfo::new(
            LogFormat::Terminal,
            LevelFilter::INFO.to_string(),
            "".to_string(),
            Some("always".to_string()),
        ))
        .init();

    let local_addr = SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 30303);

    let secret_key = SecretKey::new(&mut rand::thread_rng());

    let net_cfg = NetworkConfig::builder(secret_key)
        .boot_nodes(boot_nodes())
        .set_head(head())
        .with_pow()
        .listener_addr(local_addr)
        .eth_protocol(BscNetworkProtocol::default())
        .build(NoopProvider::eth(bsc_chain_spec()));

    let net_cfg = net_cfg.set_discovery_v4(
        Discv4ConfigBuilder::default()
            .add_boot_nodes(boot_nodes())
            .lookup_interval(Duration::from_millis(500))
            .build(),
    );
    let net_manager =
        NetworkManager::<EthNetworkPrimitives, BscNetworkProtocol>::new(net_cfg).await.unwrap();

    let net_handle = net_manager.handle().clone();
    let mut events = net_handle.event_listener();

    tokio::spawn(net_manager);

    while let Some(evt) = events.next().await {
        match evt {
            NetworkEvent::ActivePeerSession { info, .. } => {
                let SessionInfo { status, client_version, peer_id, .. } = info;
                info!(peers=%net_handle.num_connected_peers() , %peer_id, chain = %status.chain, ?client_version, "Session established with a new peer.");
            }
            NetworkEvent::Peer(PeerEvent::SessionClosed { peer_id, reason }) => {
                info!(peers=%net_handle.num_connected_peers() , %peer_id, ?reason, "Session closed.");
            }
            _ => {}
        }
    }
}

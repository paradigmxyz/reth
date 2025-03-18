//! Example for how hook into the bsc p2p network
//!
//! Run with
//!
//! ```sh
//! cargo run -p bsc-p2p
//! ```
//!
//! This launches a regular reth node overriding the engine api payload builder with our custom.
//!
//! Credits to: <https://blog.merkle.io/blog/fastest-transaction-network-eth-polygon-bsc>

use chainspec::{boot_nodes, bsc_chain_spec, head};
use handshake::BscHandshake;
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
    sync::Arc,
    time::Duration,
};
use tokio_stream::StreamExt;
use tracing::info;

mod block_import;
mod chainspec;
mod handshake;
mod upgrade_status;

#[tokio::main]
async fn main() {
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

//! Example for how hook into the bsc p2p network
//!
//! Run with
//!
//! ```not_rust
//! cargo run -p bsc-p2p
//! ```
//!
//! This launch the regular reth node overriding the engine api payload builder with our custom.
//!
//! Credits to: <https://blog.merkle.io/blog/fastest-transaction-network-eth-polygon-bsc>

#![cfg_attr(not(test), warn(unused_crate_dependencies))]

use chainspec::{boot_nodes, bsc_chain_spec};
use reth_discv4::Discv4ConfigBuilder;
use reth_network::{NetworkConfig, NetworkEvent, NetworkEventListenerProvider, NetworkManager};
use reth_network_api::PeersInfo;
use reth_primitives::{ForkHash, ForkId};
use reth_tracing::{
    tracing::info, tracing_subscriber::filter::LevelFilter, LayerInfo, LogFormat, RethTracer,
    Tracer,
};
use secp256k1::{rand, SecretKey};
use std::{
    net::{Ipv4Addr, SocketAddr},
    time::Duration,
};
use tokio_stream::StreamExt;

pub mod chainspec;

#[tokio::main]
async fn main() {
    // The ECDSA private key used to create our enode identifier.
    let secret_key = SecretKey::new(&mut rand::thread_rng());

    let _ = RethTracer::new()
        .with_stdout(LayerInfo::new(
            LogFormat::Terminal,
            LevelFilter::INFO.to_string(),
            "".to_string(),
            Some("always".to_string()),
        ))
        .init();

    // The local address we want to bind to
    let local_addr = SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 30303);

    // The network configuration
    let mut net_cfg = NetworkConfig::builder(secret_key)
        .listener_addr(local_addr)
        .build_with_noop_provider(bsc_chain_spec())
        .set_discovery_v4(
            Discv4ConfigBuilder::default()
                .add_boot_nodes(boot_nodes())
                // Set Discv4 lookup interval to 1 second
                .lookup_interval(Duration::from_secs(1))
                .build(),
        );

    // latest BSC forkId, we need to override this to allow connections from BSC nodes
    let fork_id = ForkId { hash: ForkHash([0x07, 0xb5, 0x43, 0x28]), next: 0 };
    net_cfg.fork_filter.set_current_fork_id(fork_id);
    let net_manager = NetworkManager::new(net_cfg).await.unwrap();

    // The network handle is our entrypoint into the network.
    let net_handle = net_manager.handle().clone();
    let mut events = net_handle.event_listener();

    // NetworkManager is a long running task, let's spawn it
    tokio::spawn(net_manager);
    info!("Looking for BSC peers...");

    while let Some(evt) = events.next().await {
        // For the sake of the example we only print the session established event
        // with the chain specific details
        match evt {
            NetworkEvent::SessionEstablished { status, client_version, peer_id, .. } => {
                info!(peers=%net_handle.num_connected_peers() , %peer_id, chain = %status.chain, ?client_version, "Session established with a new peer.");
            }
            NetworkEvent::SessionClosed { peer_id, reason } => {
                info!(peers=%net_handle.num_connected_peers() , %peer_id, ?reason, "Session closed.");
            }

            _ => {}
        }
    }
    // We will be disconnected from peers since we are not able to answer to network requests
}

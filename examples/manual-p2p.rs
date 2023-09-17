//! Low level example of connecting to and communicating with a peer.
//!
//! Run with
//!
//! ```not_rust
//! cargo run --example manual-p2p
//! ```

use futures::StreamExt;
use reth_discv4::{Discv4, Discv4ConfigBuilder, DEFAULT_DISCOVERY_ADDRESS, DEFAULT_DISCOVERY_PORT};
use reth_network::config::rng_secret_key;
use reth_primitives::{mainnet_nodes, NodeRecord};

#[tokio::main]
async fn main() -> eyre::Result<()> {
    // Find some peers to talk to through the discv4 protocol
    let mut config = Discv4ConfigBuilder::default();
    config.add_boot_nodes(mainnet_nodes());

    let socket = DEFAULT_DISCOVERY_ADDRESS;
    let secret_key = rng_secret_key();

    let local_enr = NodeRecord {
        id: Default::default(),
        address: socket.ip(),
        tcp_port: DEFAULT_DISCOVERY_PORT,
        udp_port: DEFAULT_DISCOVERY_PORT,
    };

    let discv4 = Discv4::spawn(socket, local_enr, secret_key, config.build()).await?;

    // get a stream of discovery updates
    let mut discoverty_stream = discv4.update_stream().await?;

    while let Some(update) = discoverty_stream.next().await {
        match update {
            reth_discv4::DiscoveryUpdate::Added(info) => {
                println!("added_node: {:?}", info)
            }
            _ => { /* ignore other actions */ }
        }
    }

    Ok(())
}

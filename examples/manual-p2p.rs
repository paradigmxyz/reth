//! Low level example of connecting to and communicating with a peer.
//!
//! Run with
//!
//! ```not_rust
//! cargo run --example manual-p2p
//! ```

use futures::StreamExt;
use reth_discv4::{Discv4, Discv4ConfigBuilder, DEFAULT_DISCOVERY_ADDRESS, DEFAULT_DISCOVERY_PORT};
use reth_ecies::{stream::ECIESStream, util::pk2id};
use reth_eth_wire::{HelloMessage, UnauthedP2PStream};
use reth_network::config::rng_secret_key;
use reth_primitives::{mainnet_nodes, NodeRecord};
use secp256k1::SECP256K1;
use tokio::net::TcpStream;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    // Find some peers to talk to through the discv4 protocol
    let mut config = Discv4ConfigBuilder::default();
    config.add_boot_nodes(mainnet_nodes());

    let this_peer_id = pk2id(&key.public_key(SECP256K1));
    let socket = DEFAULT_DISCOVERY_ADDRESS;
    let tcp_stream = TcpStream::connect(socket).await?;
    let key = rng_secret_key();

    let local_enr = NodeRecord {
        id: this_peer_id,
        address: socket.ip(),
        tcp_port: DEFAULT_DISCOVERY_PORT,
        udp_port: DEFAULT_DISCOVERY_PORT,
    };

    let discv4 = Discv4::spawn(socket, local_enr, key, config.build()).await?;

    // get a stream of discovery updates
    let mut discovery_stream = discv4.update_stream().await?;

    while let Some(update) = discovery_stream.next().await {
        if let reth_discv4::DiscoveryUpdate::Added(peer) = update {
            let sink = ECIESStream::connect(tcp_stream, key, peer.id).await.unwrap();

            let (p2p_stream, _) = UnauthedP2PStream::new(sink)
                .handshake(HelloMessage::builder(this_peer_id).build())
                .await
                .unwrap();

            println!("p2p_stream: {:?}", p2p_stream);

            //let (client_stream, _) =
            //    UnauthedEthStream::new(p2p_stream).handshake(status, fork_filter).await.unwrap();
        }
    }

    Ok(())
}

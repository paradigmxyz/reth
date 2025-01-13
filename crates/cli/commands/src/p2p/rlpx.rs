//! RLPx subcommand of P2P Debugging tool.

use std::time::Instant;

use clap::{Parser, Subcommand};
use futures::{SinkExt, StreamExt};
use reth_ecies::stream::ECIESStream;
use reth_eth_wire::{HelloMessage, UnauthedP2PStream};
use reth_network::config::rng_secret_key;
use reth_network_peers::{pk2id, AnyNode};
use secp256k1::SECP256K1;
use tokio::{
    net::TcpStream,
    sync::oneshot::error::TryRecvError,
    time::{self, Duration},
};

/// RLPx commands
#[derive(Parser, Debug)]
pub struct Command {
    #[clap(subcommand)]
    subcommand: Subcommands,
}

impl Command {
    // Execute `p2p rlpx` command.
    pub async fn execute(self) -> eyre::Result<()> {
        match self.subcommand {
            Subcommands::Ping { node } => {
                let key = rng_secret_key();
                let node_record = node
                    .node_record()
                    .ok_or_else(|| eyre::eyre!("failed to parse node {}", node))?;
                let outgoing =
                    TcpStream::connect((node_record.address, node_record.tcp_port)).await?;
                let ecies_stream = ECIESStream::connect(outgoing, key, node_record.id).await?;

                let peer_id = pk2id(&key.public_key(SECP256K1));
                let hello = HelloMessage::builder(peer_id).build();

                let (mut p2p_stream, their_hello) =
                    UnauthedP2PStream::new(ecies_stream).handshake(hello).await?;

                println!("{:#?}", their_hello);

                let mut rx = p2p_stream.subscribe_pong();

                p2p_stream.send_ping();
                p2p_stream.flush().await?;

                let start_time = Instant::now();
                let time_duration = Duration::from_secs(3);

                loop {
                    tokio::select! {
                        Some(_) = p2p_stream.next() => {
                            match rx.try_recv() {
                                Ok(_) => {
                                    println!("successfully ping");
                                    return Ok(());
                                },
                                Err(TryRecvError::Empty) => {},
                                _ => return Err(eyre::eyre!("failed to get pong from node {}", node)),
                            }
                        },
                        _ = time::sleep(Duration::from_millis(100)) => {
                            if start_time.elapsed() >= time_duration {
                                return Err(eyre::eyre!("timeout to get pong from node {}", node));
                            }
                        },
                    }
                }
            }
        }
    }
}

#[derive(Subcommand, Debug)]
enum Subcommands {
    /// ping node
    Ping {
        /// The node to ping.
        node: AnyNode,
    },
}

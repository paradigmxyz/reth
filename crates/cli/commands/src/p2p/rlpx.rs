//! RLPx subcommand of P2P Debugging tool.

use clap::{Parser, Subcommand};
use enr::Enr;
use reth_ecies::stream::ECIESStream;
use reth_eth_wire::{HelloMessage, UnauthedP2PStream};
use reth_network::config::rng_secret_key;
use reth_network_peers::{pk2id, NodeRecord};
use secp256k1::SECP256K1;
use tokio::net::TcpStream;

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
                let enr = node.parse::<Enr<secp256k1::SecretKey>>().unwrap();
                let node_record = NodeRecord::try_from(&enr)?;
                let outgoing =
                    TcpStream::connect((node_record.address, node_record.tcp_port)).await?;
                let ecies_stream = ECIESStream::connect(outgoing, key, node_record.id).await?;

                let peer_id = pk2id(&key.public_key(SECP256K1));
                let hello = HelloMessage::builder(peer_id).build();

                let (_, their_hello) =
                    UnauthedP2PStream::new(ecies_stream).handshake(hello).await?;

                println!("{:#?}", their_hello);
            }
        }
        Ok(())
    }
}

#[derive(Subcommand, Debug)]
enum Subcommands {
    /// ping node
    Ping {
        #[arg(long, short)]
        /// The node to ping.
        node: String,
    },
}

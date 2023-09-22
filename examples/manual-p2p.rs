//! Low level example of connecting to and communicating with a peer.
//!
//! Run with
//!
//! ```not_rust
//! cargo run --example manual-p2p
//! ```

use std::time::Duration;

use colored::*;
use futures::StreamExt;
use reth_discv4::{Discv4, Discv4ConfigBuilder, DEFAULT_DISCOVERY_ADDRESS, DEFAULT_DISCOVERY_PORT};
use reth_ecies::{stream::ECIESStream, util::pk2id};
use reth_eth_wire::{
    EthMessage, EthStream, HelloMessage, P2PStream, Status, UnauthedEthStream, UnauthedP2PStream,
};
use reth_network::config::rng_secret_key;
use reth_primitives::{mainnet_nodes, Chain, Hardfork, Head, NodeRecord, MAINNET, MAINNET_GENESIS};
use secp256k1::{SecretKey, SECP256K1};
use tokio::net::TcpStream;

type AuthedP2PStream = P2PStream<ECIESStream<TcpStream>>;
type AuthedEthStream = EthStream<P2PStream<ECIESStream<TcpStream>>>;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    // Setup discovery v4 protocol to find peers to talk to
    let mut discv4_cfg = Discv4ConfigBuilder::default();
    discv4_cfg.add_boot_nodes(mainnet_nodes());
    discv4_cfg.lookup_interval(Duration::from_secs(1));

    // Setup configs related to this 'node'
    let our_key = rng_secret_key();
    let our_peer_id = pk2id(&our_key.public_key(SECP256K1));
    let our_enr = NodeRecord {
        id: our_peer_id,
        address: DEFAULT_DISCOVERY_ADDRESS.ip(),
        tcp_port: DEFAULT_DISCOVERY_PORT,
        udp_port: DEFAULT_DISCOVERY_PORT,
    };

    // Start discovery protocol
    let discv4 =
        Discv4::spawn(DEFAULT_DISCOVERY_ADDRESS, our_enr, our_key, discv4_cfg.build()).await?;
    let mut discv4_stream = discv4.update_stream().await?;

    while let Some(update) = discv4_stream.next().await {
        tokio::spawn(async move {
            if let reth_discv4::DiscoveryUpdate::Added(peer) = update {
                // Boot nodes hard at work, lets not disturb them
                if mainnet_nodes().contains(&peer) {
                    return
                }

                let (p2p_stream, their_hello) = match handshake_p2p(peer, our_key).await {
                    Ok(s) => s,
                    Err(e) => {
                        #[rustfmt::skip]
                        println!("{}",format!("Failed P2P handshake with peer {}, {}", peer.address, e).yellow());
                        return
                    }
                };

                let (mut eth_stream, their_status) = match handshake_eth(p2p_stream).await {
                    Ok(s) => s,
                    Err(e) => {
                        #[rustfmt::skip]
                        println!("{}", format!("Failed ETH handshake with peer {}, {}", peer.address, e).yellow());
                        return
                    }
                };

                println!("{}", format!("Succesfully connected to a peer at {}:{} ({}) using eth-wire version eth/{}", peer.address, peer.tcp_port, their_hello.client_version, their_status.version).green());

                snoop(peer, &mut eth_stream).await;
            }
        });
    }

    Ok(())
}

// Perform a P2P handshake with a peer
async fn handshake_p2p(
    peer: NodeRecord,
    key: SecretKey,
) -> eyre::Result<(AuthedP2PStream, HelloMessage)> {
    let outgoing = TcpStream::connect((peer.address, peer.tcp_port)).await?;
    let ecies_stream = ECIESStream::connect(outgoing, key, peer.id).await?;

    let our_peer_id = pk2id(&key.public_key(SECP256K1));
    let our_hello = HelloMessage::builder(our_peer_id).build();

    Ok(UnauthedP2PStream::new(ecies_stream).handshake(our_hello).await?)
}

// Perform a ETH Wire handshake with a peer
async fn handshake_eth(p2p_stream: AuthedP2PStream) -> eyre::Result<(AuthedEthStream, Status)> {
    let fork_filter = MAINNET.fork_filter(Head {
        timestamp: MAINNET.fork(Hardfork::Shanghai).as_timestamp().unwrap(),
        ..Default::default()
    });

    let status = Status::builder()
        .chain(Chain::mainnet())
        .genesis(MAINNET_GENESIS)
        .forkid(Hardfork::Shanghai.fork_id(&MAINNET).unwrap())
        .build();

    let status = Status { version: p2p_stream.shared_capability().version(), ..status };
    let eth_unauthed = UnauthedEthStream::new(p2p_stream);
    Ok(eth_unauthed.handshake(status, fork_filter).await?)
}

// Snoop by greedily capturing all peer's broadcasts
// note: this node cannot handle request so will be disconnected by peer when challenged
#[rustfmt::skip]
async fn snoop(peer: NodeRecord, eth_stream: &mut AuthedEthStream) {
    while let Some(Ok(update)) = eth_stream.next().await {
        match update {
            EthMessage::NewBlockHashes(block_hashes) => {
                println!("{}", format!("Got {} new block hashes from peer {}", block_hashes.0.len(), peer.address).cyan().bold());
            }
            EthMessage::NewBlock(block) => {
                println!("{}", format!("Got new block data {:?} from peer {}", block, peer.address).cyan().bold());
            }
            EthMessage::NewPooledTransactionHashes66(txs) => {
                println!("{}", format!("Got {} new tx hashes from peer {}", txs.0.len(), peer.address).cyan().bold());
            }
            EthMessage::NewPooledTransactionHashes68(txs) => {
                println!("{}", format!("Got {} new tx hashes from peer {}", txs.hashes.len(), peer.address).cyan().bold());
            }
            EthMessage::GetBlockHeaders(_) => {
                println!("{}", format!("Unable to serve the GetBlockHeaders request from peer {}", peer.address).red().bold());
            },
            EthMessage::GetBlockBodies(_) => {
                println!("{}", format!("Unable to serve the GetBlockBodies request from peer {}", peer.address).red().bold());
            },
            EthMessage::GetPooledTransactions(_) => {
                println!("{}", format!("Unable to serve the GetPooledTransactions request from peer {}", peer.address).red().bold());
            },
            EthMessage::GetNodeData(_) => {
                println!("{}", format!("Unable to serve the GetNodeData request from peer {}", peer.address).red().bold());
            },
            EthMessage::GetReceipts(_) => {
                println!("{}", format!("Unable to serve the GetReceipts request from peer {}", peer.address).red().bold());
            },
            _ => {},
        }
    }
}

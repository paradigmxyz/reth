//! Low level example of connecting to and communicating with a peer.
//!
//! Run with
//!
//! ```sh
//! cargo run -p manual-p2p
//! ```

#![warn(unused_crate_dependencies)]

use std::time::Duration;

use alloy_consensus::constants::MAINNET_GENESIS_HASH;
use futures::StreamExt;
use reth_discv4::{DiscoveryUpdate, Discv4, Discv4ConfigBuilder, DEFAULT_DISCOVERY_ADDRESS};
use reth_ecies::stream::ECIESStream;
use reth_ethereum::{
    chainspec::{Chain, EthereumHardfork, Head, MAINNET},
    network::{
        config::rng_secret_key,
        eth_wire::{
            EthMessage, EthStream, HelloMessage, P2PStream, UnauthedEthStream, UnauthedP2PStream,
            UnifiedStatus,
        },
        EthNetworkPrimitives,
    },
};
use reth_network_peers::{mainnet_nodes, pk2id, NodeRecord};
use secp256k1::{SecretKey, SECP256K1};
use std::sync::LazyLock;
use tokio::net::TcpStream;
use tokio::time::timeout;
use tracing::{info, warn};

type AuthedP2PStream = P2PStream<ECIESStream<TcpStream>>;
type AuthedEthStream = EthStream<P2PStream<ECIESStream<TcpStream>>, EthNetworkPrimitives>;

pub static MAINNET_BOOT_NODES: LazyLock<Vec<NodeRecord>> = LazyLock::new(mainnet_nodes);

const LOOKUP_INTERVAL: Duration = Duration::from_secs(1);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::main]
async fn main() -> eyre::Result<()> {
    tracing_subscriber::fmt::init();

    // Setup configs related to this 'node' by creating a new random
    let our_key = rng_secret_key();
    let our_enr = NodeRecord::from_secret_key(DEFAULT_DISCOVERY_ADDRESS, &our_key);

    // Setup discovery v4 protocol to find peers to talk to
    let mut discv4_cfg = Discv4ConfigBuilder::default();
    discv4_cfg.add_boot_nodes(&*MAINNET_BOOT_NODES).lookup_interval(LOOKUP_INTERVAL);

    // Start discovery protocol
    let discv4 = Discv4::spawn(our_enr.udp_addr(), our_enr, our_key, discv4_cfg.build()).await?;
    let mut discv4_stream = discv4.update_stream().await?;

    while let Some(update) = discv4_stream.next().await {
        if let DiscoveryUpdate::Added(peer) = update {
            // Boot nodes hard at work, lets not disturb them
            if MAINNET_BOOT_NODES.contains(&peer) {
                continue
            }

            let (p2p_stream, their_hello) = match handshake_p2p(peer, our_key).await {
                Ok(s) => s,
                Err(e) => {
                    warn!("Failed P2P handshake with peer {}, {}", peer.address, e);
                    continue
                }
            };

            let (eth_stream, their_status) = match handshake_eth(p2p_stream).await {
                Ok(s) => s,
                Err(e) => {
                    warn!("Failed ETH handshake with peer {}, {}", peer.address, e);
                    continue
                }
            };

            info!(
                "Successfully connected to a peer at {}:{} ({}) using eth-wire version eth/{}",
                eth_stream.peer_addr().ip(), 
                eth_stream.peer_addr().port(), 
                their_hello.client_version, 
                their_status.version
            );

            snoop(eth_stream).await;
        }
    }

    Ok(())
}

// Perform a P2P handshake with a peer
async fn handshake_p2p(
    peer: NodeRecord,
    key: SecretKey,
) -> eyre::Result<(AuthedP2PStream, HelloMessage)> {
    let outgoing = timeout(CONNECT_TIMEOUT, TcpStream::connect((peer.address, peer.tcp_port))).await??;
    let ecies_stream = ECIESStream::connect(outgoing, key, peer.id).await?;

    let our_peer_id = pk2id(&key.public_key(SECP256K1));
    let our_hello = HelloMessage::builder(our_peer_id).build();

    Ok(UnauthedP2PStream::new(ecies_stream).handshake(our_hello).await?)
}

// Perform a ETH Wire handshake with a peer
async fn handshake_eth(
    p2p_stream: AuthedP2PStream,
) -> eyre::Result<(AuthedEthStream, UnifiedStatus)> {
    let fork_filter = MAINNET.fork_filter(Head {
        timestamp: MAINNET.fork(EthereumHardfork::Shanghai).as_timestamp().unwrap(),
        ..Default::default()
    });

    let mut status = UnifiedStatus::builder()
        .chain(Chain::mainnet())
        .genesis(MAINNET_GENESIS_HASH)
        .forkid(MAINNET.hardfork_fork_id(EthereumHardfork::Shanghai).unwrap())
        .build();

    status.version = p2p_stream.shared_capabilities().eth()?.version().try_into()?;

    let eth_unauthed = UnauthedEthStream::new(p2p_stream);
    Ok(eth_unauthed.handshake(status, fork_filter).await?)
}

// Snoop by greedily capturing all broadcasts that the peer emits
// note: this node cannot handle request so it will be disconnected by peer when challenged
async fn snoop(mut eth_stream: AuthedEthStream) {
    while let Some(Ok(update)) = eth_stream.next().await {
        match update {
            EthMessage::NewPooledTransactionHashes66(txs) => log_count("tx hashes", txs.0.len(), &eth_stream),
            EthMessage::NewBlock(block) => {
                info!("Got new block data {:?} from peer {}", block, eth_stream.peer_addr());
            }
            EthMessage::NewPooledTransactionHashes68(txs) => log_count("tx hashes", txs.hashes.len(), &eth_stream),
            EthMessage::NewBlockHashes(block_hashes) => log_count("block hashes", block_hashes.0.len(), &eth_stream),
            EthMessage::GetNodeData(_)
            | EthMessage::GetReceipts(_)
            | EthMessage::GetBlockHeaders(_)
            | EthMessage::GetBlockBodies(_)
            | EthMessage::GetPooledTransactions(_) => {
                warn!("Unable to serve request from peer {}", eth_stream.peer_addr());
            }
            _ => {}
        }
    }
}

fn log_count(label: &str, count: usize, eth_stream: &AuthedEthStream) {
    info!("Got {} {} from peer {}", count, label, eth_stream.peer_addr());
}

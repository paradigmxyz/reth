use crate::{geoip::GeoIpResolver, metrics, types::NodeInfo};
use futures::StreamExt;
use reth_chainspec::{ChainSpec, EthChainSpec};
use reth_discv4::{DiscoveryUpdate, Discv4, Discv4ConfigBuilder, DEFAULT_DISCOVERY_ADDRESS};
use reth_ecies::stream::ECIESStream;
use reth_ethereum::{
    chainspec::{Chain, EthereumHardfork, Head},
    network::{
        config::rng_secret_key,
        eth_wire::{HelloMessage, P2PStream, UnauthedEthStream, UnauthedP2PStream, UnifiedStatus},
        EthNetworkPrimitives,
    },
};
use reth_network_peers::{pk2id, NodeRecord};
use secp256k1::{SecretKey, SECP256K1};
use std::{
    collections::HashSet,
    net::IpAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{net::TcpStream, sync::Semaphore};

type AuthedP2PStream = P2PStream<ECIESStream<TcpStream>>;

pub struct CrawlerConfig {
    pub chainspec: Arc<ChainSpec>,
    pub bootnodes: Vec<NodeRecord>,
    pub workers: usize,
}

pub struct Crawler {
    config: CrawlerConfig,
    geoip: Arc<GeoIpResolver>,
    key: SecretKey,
}

impl Crawler {
    pub fn new(config: CrawlerConfig, geoip: GeoIpResolver) -> Self {
        Self { config, geoip: Arc::new(geoip), key: rng_secret_key() }
    }

    pub const fn secret_key(&self) -> SecretKey {
        self.key
    }

    pub async fn run(
        &self,
        tx: tokio::sync::mpsc::Sender<NodeInfo>,
        stop_rx: tokio::sync::watch::Receiver<bool>,
    ) -> eyre::Result<()> {
        let our_enr = NodeRecord::from_secret_key(DEFAULT_DISCOVERY_ADDRESS, &self.key);

        let bootnodes = if self.config.bootnodes.is_empty() {
            self.config.chainspec.bootnodes().unwrap_or_default()
        } else {
            self.config.bootnodes.clone()
        };

        let mut discv4_cfg = Discv4ConfigBuilder::default();
        discv4_cfg.add_boot_nodes(bootnodes.clone()).lookup_interval(Duration::from_secs(1));

        let discv4 = Discv4::spawn(our_enr.udp_addr(), our_enr, self.key, discv4_cfg.build()).await?;
        let mut discv4_stream = discv4.update_stream().await?;

        let semaphore = Arc::new(Semaphore::new(self.config.workers));
        let active_workers = Arc::new(AtomicU64::new(0));
        let seen_nodes: Arc<tokio::sync::Mutex<HashSet<alloy_primitives::B512>>> =
            Arc::new(tokio::sync::Mutex::new(HashSet::new()));
        let bootnodes_set: HashSet<_> = bootnodes.into_iter().collect();

        loop {
            tokio::select! {
                update = discv4_stream.next() => {
                    let Some(update) = update else { break };

                    if let DiscoveryUpdate::Added(peer) = update {
                        if bootnodes_set.contains(&peer) {
                            continue;
                        }

                        {
                            let mut seen = seen_nodes.lock().await;
                            if seen.contains(&peer.id) {
                                continue;
                            }
                            seen.insert(peer.id);
                        }

                        metrics::inc_discovered();

                        let permit = semaphore.clone().acquire_owned().await?;
                        let tx = tx.clone();
                        let key = self.key;
                        let geoip = self.geoip.clone();
                        let chainspec = self.config.chainspec.clone();
                        let active = active_workers.clone();

                        active.fetch_add(1, Ordering::Relaxed);
                        metrics::set_active_workers(active.load(Ordering::Relaxed));

                        tokio::spawn(async move {
                            match handshake_peer(peer, key, &chainspec, geoip.as_ref()).await {
                                Ok(info) => {
                                    metrics::inc_handshake_success();
                                    tracing::info!(
                                        ip = %info.ip,
                                        client = %info.client_version,
                                        country = info.country_code.as_deref().unwrap_or("??"),
                                        "Discovered node"
                                    );
                                    let _ = tx.send(info).await;
                                }
                                Err(e) => {
                                    metrics::inc_handshake_failed();
                                    tracing::debug!("Failed handshake with {}: {}", peer.address, e);
                                }
                            }
                            active.fetch_sub(1, Ordering::Relaxed);
                            metrics::set_active_workers(active.load(Ordering::Relaxed));
                            drop(permit);
                        });
                    }
                }
                result = async {
                    let mut rx = stop_rx.clone();
                    rx.changed().await
                } => {
                    if result.is_ok() && *stop_rx.borrow() {
                        break;
                    }
                }
            }
        }

        Ok(())
    }
}

async fn handshake_peer(
    peer: NodeRecord,
    key: SecretKey,
    chainspec: &ChainSpec,
    geoip: &GeoIpResolver,
) -> eyre::Result<NodeInfo> {
    let timeout = Duration::from_secs(10);

    let (p2p_stream, hello) = tokio::time::timeout(timeout, handshake_p2p(peer, key)).await??;

    let eth_status = tokio::time::timeout(timeout, handshake_eth(p2p_stream, chainspec))
        .await
        .ok()
        .and_then(|r| r.ok());

    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let ip: IpAddr = peer.address;
    let country_code = geoip.lookup(ip);

    let capabilities: Vec<String> =
        hello.capabilities.iter().map(|c| format!("{}/{}", c.name, c.version)).collect();

    let eth_version = eth_status.as_ref().map(|s| s.version as u8);
    let chain_id = eth_status.as_ref().map(|s| s.chain.id());

    Ok(NodeInfo {
        node_id: peer.id,
        enode: format!("enode://{}@{}:{}", peer.id, peer.address, peer.tcp_port),
        ip,
        tcp_port: peer.tcp_port,
        udp_port: peer.udp_port,
        client_version: hello.client_version,
        capabilities,
        eth_version,
        chain_id,
        country_code,
        first_seen: now,
        last_seen: now,
        last_error: None,
        last_checked: Some(now),
        is_alive: true,
        consecutive_failures: 0,
    })
}

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

async fn handshake_eth(p2p_stream: AuthedP2PStream, chainspec: &ChainSpec) -> eyre::Result<UnifiedStatus> {
    let fork_filter = chainspec.fork_filter(Head {
        timestamp: chainspec.fork(EthereumHardfork::Shanghai).as_timestamp().unwrap_or(0),
        ..Default::default()
    });

    let unified_status = UnifiedStatus::builder()
        .chain(Chain::from_id(chainspec.chain_id()))
        .genesis(chainspec.genesis_hash())
        .forkid(chainspec.hardfork_fork_id(EthereumHardfork::Shanghai).unwrap())
        .build();

    let eth_version = p2p_stream
        .shared_capabilities()
        .eth_version()
        .map_err(|_| eyre::eyre!("No ETH capability"))?;

    let status = UnifiedStatus { version: eth_version, ..unified_status };

    let eth_unauthed = UnauthedEthStream::new(p2p_stream);
    let (_, their_status) =
        eth_unauthed.handshake::<EthNetworkPrimitives>(status, fork_filter).await?;
    Ok(their_status)
}

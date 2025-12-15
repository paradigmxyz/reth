use crate::{geoip::GeoIpResolver, metrics, storage::Storage};
use reth_chainspec::ChainSpec;
use reth_ecies::stream::ECIESStream;
use reth_ethereum::{
    chainspec::{Chain, EthereumHardfork, Head},
    network::{
        eth_wire::{HelloMessage, P2PStream, UnauthedEthStream, UnauthedP2PStream, UnifiedStatus},
        EthNetworkPrimitives,
    },
};
use reth_network_peers::{pk2id, NodeRecord};
use secp256k1::{SecretKey, SECP256K1};
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{net::TcpStream, sync::Semaphore};

type AuthedP2PStream = P2PStream<ECIESStream<TcpStream>>;

pub struct RecheckConfig {
    pub chainspec: Arc<ChainSpec>,
    pub batch_size: u32,
    pub max_age_secs: u64,
    pub check_interval: Duration,
    pub max_failures: u32,
    pub workers: usize,
}

pub struct Rechecker {
    config: RecheckConfig,
    storage: Storage,
    geoip: Arc<GeoIpResolver>,
    key: SecretKey,
}

impl Rechecker {
    pub fn new(config: RecheckConfig, storage: Storage, geoip: GeoIpResolver, key: SecretKey) -> Self {
        Self { config, storage, geoip: Arc::new(geoip), key }
    }

    pub async fn run(&self, mut stop_rx: tokio::sync::watch::Receiver<bool>) -> eyre::Result<()> {
        let semaphore = Arc::new(Semaphore::new(self.config.workers));
        let active_workers = Arc::new(AtomicU64::new(0));

        loop {
            tokio::select! {
                _ = tokio::time::sleep(self.config.check_interval) => {
                    self.recheck_batch(&semaphore, &active_workers).await?;
                }
                result = stop_rx.changed() => {
                    if result.is_ok() && *stop_rx.borrow() {
                        break;
                    }
                }
            }
        }

        Ok(())
    }

    async fn recheck_batch(
        &self,
        semaphore: &Arc<Semaphore>,
        active_workers: &Arc<AtomicU64>,
    ) -> eyre::Result<()> {
        let stale_nodes = self
            .storage
            .get_stale_nodes(self.config.batch_size, self.config.max_age_secs)
            .await?;

        if stale_nodes.is_empty() {
            return Ok(());
        }

        tracing::info!(count = stale_nodes.len(), "Rechecking stale nodes");

        let mut handles = Vec::new();

        for stale in stale_nodes {
            let permit = semaphore.clone().acquire_owned().await?;
            let storage = self.storage.clone();
            let geoip = self.geoip.clone();
            let chainspec = self.config.chainspec.clone();
            let key = self.key;
            let max_failures = self.config.max_failures;
            let active = active_workers.clone();
            let node_record = stale.to_node_record();

            active.fetch_add(1, Ordering::Relaxed);

            let handle = tokio::spawn(async move {
                let result = recheck_peer(node_record, key, &chainspec).await;

                match result {
                    Ok(hello) => {
                        let country = geoip.lookup(stale.ip);
                        if let Err(e) = storage
                            .update_node_alive(&stale.node_id, &hello.client_version, country.as_deref())
                            .await
                        {
                            tracing::error!("Failed to update node: {}", e);
                        } else {
                            metrics::inc_recheck_success();
                            tracing::debug!(
                                ip = %stale.ip,
                                client = %hello.client_version,
                                "Node still alive"
                            );
                        }
                    }
                    Err(e) => {
                        let error_msg = e.to_string();
                        if let Err(e) = storage
                            .increment_failure(&stale.node_id, &error_msg, max_failures)
                            .await
                        {
                            tracing::error!("Failed to update node failure: {}", e);
                        } else {
                            metrics::inc_recheck_failed();
                            tracing::debug!(ip = %stale.ip, error = %error_msg, "Node unreachable");
                        }
                    }
                }

                active.fetch_sub(1, Ordering::Relaxed);
                drop(permit);
            });

            handles.push(handle);
        }

        for handle in handles {
            let _ = handle.await;
        }

        if let Ok(alive) = self.storage.count_alive_nodes().await {
            metrics::set_alive_nodes(alive as u64);
        }

        Ok(())
    }
}

async fn recheck_peer(
    peer: NodeRecord,
    key: SecretKey,
    chainspec: &ChainSpec,
) -> eyre::Result<HelloMessage> {
    let timeout = Duration::from_secs(10);

    let (p2p_stream, hello) = tokio::time::timeout(timeout, handshake_p2p(peer, key)).await??;

    // Also do ETH handshake to verify they're on the right chain
    let _ = tokio::time::timeout(timeout, handshake_eth(p2p_stream, chainspec)).await;

    Ok(hello)
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

async fn handshake_eth(
    p2p_stream: AuthedP2PStream,
    chainspec: &ChainSpec,
) -> eyre::Result<UnifiedStatus> {
    use reth_chainspec::EthChainSpec;

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

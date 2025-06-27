use crate::{
    connection::ConnWrapper,
    error::EthStatsError,
    events::{AuthMsg, BlockStats, HistoryMsg, LatencyMsg, NodeInfo, NodeStats, PendingMsg, PendingStats, PingMsg, StatsMsg},
};
use reth_network_api::{NetworkInfo, Peers};
use reth_provider::{BlockReaderIdExt, CanonStateSubscriptions};
use reth_transaction_pool::TransactionPool;

use serde_json::{Value};
use std::{
    str::FromStr, sync::Arc, time::{Duration, Instant}
};
use tokio::{
    sync::{mpsc, Mutex, RwLock},
    time::{interval, sleep, timeout},
};
use tokio_tungstenite::connect_async;
use url::Url;
use chrono::Local;

const RECONNECT_INTERVAL: Duration = Duration::from_secs(5);
const PING_TIMEOUT: Duration = Duration::from_secs(5);
const REPORT_INTERVAL: Duration = Duration::from_secs(15);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const READ_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct EthstatsCredentials {
    pub node_id: String,
    pub secret: String,
    pub host: String,
}

impl FromStr for EthstatsCredentials {
    type Err = EthStatsError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Expecting format: node_id:secret@host
        let parts: Vec<&str> = s.split('@').collect();
        if parts.len() != 2 {
            return Err(EthStatsError::InvalidUrl("Missing '@' separator".to_string()));
        }
        let creds = parts[0];
        let host = parts[1].to_string();
        let creds_parts: Vec<&str> = creds.split(':').collect();
        if creds_parts.len() != 2 {
            return Err(EthStatsError::InvalidUrl(
                "Missing ':' separator in credentials".to_string(),
            ));
        }
        let node_id = creds_parts[0].to_string();
        let secret = creds_parts[1].to_string();

        Ok(EthstatsCredentials { node_id, secret, host })
    }
}

#[derive(Debug)]
pub struct EthStatsService<Network, Provider, Pool> {
    credentials: EthstatsCredentials,
    conn: Arc<RwLock<Option<ConnWrapper>>>,
    last_ping: Arc<Mutex<Option<Instant>>>,
    network: Network,
    provider: Provider,
    pool: Pool
}

impl<Network, Provider, Pool> EthStatsService<Network, Provider, Pool>
where
    Network: NetworkInfo + Peers,
    Provider: BlockReaderIdExt + CanonStateSubscriptions,
    Pool: TransactionPool
{
    pub async fn new(url: &str, network: Network, provider: Provider, pool: Pool) -> Result<Self, EthStatsError> {
        let credentials = EthstatsCredentials::from_str(url)?;
        let service = EthStatsService {
            credentials,
            conn: Arc::new(RwLock::new(None)),
            last_ping: Arc::new(Mutex::new(None)),
            network,
            provider,
            pool
        };
        service.connect().await?;

        Ok(service)
    }

    async fn connect(&self) -> Result<(), EthStatsError> {
        let full_url = format!("ws://{}/api", self.credentials.host);
        let url = Url::parse(&full_url)
            .map_err(|e| EthStatsError::InvalidUrl(format!("Invalid URL: {} - {}", full_url, e)))?;

        match timeout(CONNECT_TIMEOUT, connect_async(url.to_string())).await {
            Ok(Ok((ws_stream, _))) => {
                let conn: ConnWrapper = ConnWrapper::new(ws_stream);
                *self.conn.write().await = Some(conn.clone());
                self.login().await?;
                Ok(())
            }
            Ok(Err(e)) => Err(EthStatsError::InvalidUrl(e.to_string())),
            Err(_) => Err(EthStatsError::Timeout),
        }
    }

    async fn login(&self) -> Result<(), EthStatsError> {
        let conn = self.conn.read().await;
        let conn = conn.as_ref().ok_or(EthStatsError::NotConnected)?;

        let network_status = self.network.network_status().await.map_err(|e| EthStatsError::AuthError(e.to_string()))?;
        let id = &self.credentials.node_id;
        let secret = &self.credentials.secret;
        let protocol = format!("eth/{}", network_status.protocol_version);

        let auth = AuthMsg {
            id: id.clone(),
            secret: secret.clone(),
            info: NodeInfo {
                name: "Reth".to_string(),
                node: id.clone(),
                port: 30303,
                network: self.network.chain_id().to_string(),
                protocol,
                api: "No".to_string(),
                os: std::env::consts::OS.into(),
                os_ver: std::env::consts::ARCH.into(),
                client: network_status.client_version.clone(),
                history: true,
            },
        };

        let message = auth.generate_login_message();
        conn.write_json(&message).await?;

        let response =
            timeout(READ_TIMEOUT, conn.read_json()).await.map_err(|_| EthStatsError::Timeout)??;

        if let Some(ack) = response.get("emit") {
            if ack.get(0) == Some(&Value::String("ready".to_string())) {
                tracing::info!("Login successful to ethstats server");
                return Ok(());
            }
        }

        Err(EthStatsError::AuthError("Unauthorized or unexpected login response".into()))
    }

    async fn report_stats(&self) -> Result<(), EthStatsError> {
        let conn = self.conn.read().await;
        let conn = conn.as_ref().ok_or(EthStatsError::NotConnected)?;

        let stats_msg = StatsMsg {
            id: self.credentials.node_id.clone(),
            stats: NodeStats {
                active: true,
                syncing: self.network.is_syncing(),
                peers: self.network.num_connected_peers() as i32,
                gas_price: 0, // TODO
                uptime: 100
            }
        };

        let message = stats_msg.generate_stats_message();
        conn.write_json(&message).await?;

        Ok(())
    }

    async fn send_ping(&self) -> Result<(), EthStatsError> {
        let conn = self.conn.read().await;
        let conn = conn.as_ref().ok_or(EthStatsError::NotConnected)?;

        let ping_time = Instant::now();
        *self.last_ping.lock().await = Some(ping_time);

        let client_time = Local::now().format("%Y-%m-%d %H:%M:%S%.f %:z %Z").to_string();
        let ping_msg = PingMsg {
            id: self.credentials.node_id.clone(),
            client_time
        };

        let message = ping_msg.generate_ping_message();
        conn.write_json(&message).await?;

        // Start ping timeout
        let active_ping = self.last_ping.clone();
        let conn_ref = self.conn.clone();
        tokio::spawn(async move {
            sleep(PING_TIMEOUT).await;
            let mut active = active_ping.lock().await;
            if active.is_some() {
                tracing::warn!("Ping timeout");
                *active = None;
                // Clear connection to trigger reconnect
                if let Some(conn) = conn_ref.write().await.take() {
                    let _ = conn.close().await;
                }
            }
        });

        Ok(())
    }

    async fn report_latency(&self) -> Result<(), EthStatsError> {
        let mut active = self.last_ping.lock().await;
        if let Some(start) = active.take() {
            let latency = start.elapsed().as_millis() as u64 / 2;
            
            tracing::debug!("Pong received, latency: {}ms", latency);
            
            let conn = self.conn.read().await;
            let conn = conn.as_ref().ok_or(EthStatsError::NotConnected)?;

            let latency_msg = LatencyMsg {
                id: self.credentials.node_id.clone(),
                latency
            };
    
            let message = latency_msg.generate_latency_message();
            conn.write_json(&message).await?
        }

        Ok(())
    }

    async fn report_pending(&self) -> Result<(), EthStatsError> {
        let conn = self.conn.read().await;
        let conn = conn.as_ref().ok_or(EthStatsError::NotConnected)?;
        let pending = self.pool.pool_size().pending as i32;

        let pending_msg = PendingMsg {
            id: self.credentials.node_id.clone(),
            stats: PendingStats {
                pending
            }
        };

        let message = pending_msg.generate_pending_message();
        conn.write_json(&message).await?;

        tracing::debug!("Pending handled, pending txs: {}", pending);

        Ok(())
    }

    async fn handle_history_request(&self) -> Result<(), EthStatsError> {
        let conn = self.conn.read().await;
        let conn = conn.as_ref().ok_or(EthStatsError::NotConnected)?;

        // In a real implementation, this would fetch actual block history
        let history = HistoryMsg {
            id: self.credentials.node_id.clone(),
            history: vec![
                // BlockStats {
                //     number: 123455,
                //     hash: "0x123...".into(),
                //     parent: "0xabc...".into(),
                //     timestamp: 1678899990,
                //     gas_used: 14900000,
                //     gas_limit: 30000000,
                //     miner: "0xminer...".into(),
                //     txs: vec!["0xtx1...".into()],
                // },
                // BlockStats {
                //     number: 123454,
                //     hash: "0x456...".into(),
                //     parent: "0xdef...".into(),
                //     timestamp: 1678899980,
                //     gas_used: 14800000,
                //     gas_limit: 30000000,
                //     miner: "0xminer...".into(),
                //     txs: vec!["0xtx3...".into(), "0xtx4...".into()],
                // },
            ],
        };

        let message = history.generate_history_message();
        conn.write_json(&message).await?;
        tracing::debug!("History request handled");

        Ok(())
    }

    async fn report(&self) -> Result<(), EthStatsError> {
        self.send_ping().await?;
        self.report_pending().await?;
        self.report_stats().await?;

        Ok(())
    }

    async fn handle_message(&self, msg: Value) -> Result<(), EthStatsError> {
        if let Some(emit) = msg.get("emit") {
            if let Some(Value::String(command)) = emit.get(0) {
                match command.as_str() {
                    "node-pong" => {
                        self.report_latency().await?;
                    }
                    "history" => {
                        // self.handle_history_request().await?;
                    }
                    other => tracing::warn!("Unhandled command: {}", other),
                }
            }
        } else {
            tracing::warn!("Stats server sent non-broadcast, msg {}", msg);
        }

        Ok(())
    }

    pub async fn run(self) {
        // Create channels for internal communication
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel(1);
        let (message_tx, mut message_rx) = mpsc::channel(32);

        // Start the read loop in a separate task
        let read_handle = {
            let conn = self.conn.clone();
            let message_tx = message_tx.clone();
            let shutdown_tx = shutdown_tx.clone();

            tokio::spawn(async move {
                loop {
                    let conn = conn.read().await;
                    if let Some(conn) = conn.as_ref() {
                        match timeout(READ_TIMEOUT, conn.read_json()).await {
                            Ok(Ok(msg)) => {
                                if message_tx.send(msg).await.is_err() {
                                    break;
                                }
                            }
                            Ok(Err(e)) => {
                                tracing::error!("Read error: {}", e);
                                break;
                            }
                            Err(_) => {
                                tracing::warn!("Read timeout");
                                break;
                            }
                        }
                    } else {
                        sleep(RECONNECT_INTERVAL).await;
                    }
                }

                let _ = shutdown_tx.send(()).await;
            })
        };

        // Set up intervals
        let mut report_interval = interval(REPORT_INTERVAL);
        let mut reconnect_interval = interval(RECONNECT_INTERVAL);

        // Main event loop using select!
        loop {
            tokio::select! {
                // Handle shutdown signal
                _ = shutdown_rx.recv() => {
                    tracing::info!("Shutting down ethstats service");
                    break;
                }

                // Handle messages from the read loop
                Some(msg) = message_rx.recv() => {
                    if let Err(e) = self.handle_message(msg).await {
                        tracing::error!("Error handling message: {}", e);
                        self.disconnect().await;
                    }
                }

                // Handle stats reporting
                _ = report_interval.tick() => {
                    if let Err(e) = self.report().await {
                        tracing::error!("Failed to report: {}", e);
                        self.disconnect().await;
                    }
                }

                // Handle reconnection
                _ = reconnect_interval.tick(), if self.conn.read().await.is_none() => {
                    match self.connect().await {
                        Ok(_) => tracing::info!("Reconnected successfully"),
                        Err(e) => tracing::error!("Reconnect failed: {}", e),
                    }
                }
            }
        }

        // Cleanup
        self.disconnect().await;

        // Cancel background tasks
        read_handle.abort();
    }

    async fn disconnect(&self) {
        if let Some(conn) = self.conn.write().await.take() {
            if let Err(e) = conn.close().await {
                tracing::error!("Error closing connection: {}", e);
            }
        }
    }
}

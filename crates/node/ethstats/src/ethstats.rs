use crate::{
    connection::ConnWrapper,
    error::EthStatsError,
    events::{AuthMsg, BlockStats, HistoryMsg, LatencyMsg, NodeInfo, StatsMsg},
};

use serde_json::{json, Value};
use std::{
    str::FromStr,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    sync::{mpsc, Mutex, RwLock},
    time::{interval, sleep, timeout},
};
use tokio_tungstenite::connect_async;
use url::Url;

const RECONNECT_INTERVAL: Duration = Duration::from_secs(5);
const PING_INTERVAL: Duration = Duration::from_secs(15);
const STATS_INTERVAL: Duration = Duration::from_secs(15);
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

pub struct EthStatsService {
    credentials: EthstatsCredentials,
    conn: Arc<RwLock<Option<ConnWrapper>>>,
    last_ping: Arc<Mutex<Option<Instant>>>,
    latency: Arc<Mutex<u64>>,
    // Add Reth-specific data sources here:
    // network: NetworkHandle,
    // blockchain: Blockchain,
}

impl EthStatsService {
    pub async fn new(url: &str) -> Result<Self, EthStatsError> {
        let credentials = EthstatsCredentials::from_str(url)?;
        let service = EthStatsService {
            credentials,
            conn: Arc::new(RwLock::new(None)),
            last_ping: Arc::new(Mutex::new(None)),
            latency: Arc::new(Mutex::new(0)),
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

        let auth = AuthMsg {
            id: self.credentials.node_id.clone(),
            secret: self.credentials.secret.clone(),
            info: NodeInfo {
                name: self.credentials.node_id.clone(),
                node: "reth".into(),
                port: 30303,
                network: "1".into(),
                protocol: "eth/68".into(),
                api: "No".into(),
                os: std::env::consts::OS.into(),
                os_ver: std::env::consts::OS.into(),
                client: env!("CARGO_PKG_VERSION").into(),
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

        // In a real implementation, these would come from Reth components
        let stats_msg = StatsMsg {
            // id: self.credentials.node_id.clone(),
            // active: true,
            // syncing: false,
            // mining: false,
            // hashrate: 0,
            // peers: 10, // Replace with actual peer count
            // gas_price: 123456789,
            // uptime: 100,
            // block: BlockStats {
            //     number: 123456, // Replace with latest block
            //     hash: "0x123...".into(),
            //     parent: "0xabc...".into(),
            //     timestamp: 1678900000,
            //     gas_used: 15000000,
            //     gas_limit: 30000000,
            //     miner: "0xminer...".into(),
            //     txs: vec!["0xtx1...".into(), "0xtx2...".into()],
            // },
        };

        let message = stats_msg.generate_stats_message();
        conn.write_json(&message).await?;

        Ok(())
    }

    async fn handle_message(&self, msg: Value) -> Result<(), EthStatsError> {
        if let Some(emit) = msg.get("emit") {
            if let Some(Value::String(command)) = emit.get(0) {
                match command.as_str() {
                    "node-ping" => {
                        self.handle_ping().await?;
                    }
                    "history" => {
                        self.handle_history_request().await?;
                    }
                    "block" => {
                        // In a real implementation, this would update internal state
                        tracing::debug!("Block update received");
                    }
                    "pending" => {
                        // In a real implementation, this would update internal state
                        tracing::debug!("Pending update received");
                    }
                    other => tracing::warn!("Unhandled command: {}", other),
                }
            }
        }

        Ok(())
    }

    async fn handle_ping(&self) -> Result<(), EthStatsError> {
        let conn: tokio::sync::RwLockReadGuard<'_, Option<ConnWrapper>> = self.conn.read().await;
        let conn = conn.as_ref().ok_or(EthStatsError::NotConnected)?;

        let ping_time = Instant::now();
        *self.last_ping.lock().await = Some(ping_time);

        let pong = json!({"emit": ["node-pong", 1]});
        conn.write_json(&pong.to_string()).await?;

        // Calculate latency when we get the response
        let latency = ping_time.elapsed().as_millis() as u64;
        *self.latency.lock().await = latency;

        let latency_msg = LatencyMsg { id: self.credentials.node_id.clone(), latency };

        let message = latency_msg.generate_latency_message();
        conn.write_json(&message).await?;

        tracing::debug!("Ping handled, latency: {}ms", latency);

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

    pub async fn run(self) {
        // Create channels for internal communication
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel(1);
        let (message_tx, mut message_rx) = mpsc::channel(32);
        let (ping_tx, mut ping_rx) = mpsc::channel(1);

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

                shutdown_tx.send(()).await;
            })
        };

        // Start the ping monitor in a separate task
        let ping_handle = {
            let last_ping = self.last_ping.clone();
            tokio::spawn(async move {
                let mut interval = interval(PING_INTERVAL);
                loop {
                    interval.tick().await;
                    if let Some(last_ping) = *last_ping.lock().await {
                        if last_ping.elapsed() > PING_INTERVAL * 2 {
                            ping_tx.send(()).await;
                        }
                    }
                }
            })
        };

        // Set up intervals
        let mut stats_interval = interval(STATS_INTERVAL);
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

                // Handle ping timeout
                _ = ping_rx.recv() => {
                    tracing::warn!("Ping timeout, reconnecting");
                    self.disconnect().await;
                }

                // Handle stats reporting
                _ = stats_interval.tick() => {
                    if let Err(e) = self.report_stats().await {
                        tracing::error!("Failed to report stats: {}", e);
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
        ping_handle.abort();
    }

    async fn disconnect(&self) {
        if let Some(conn) = self.conn.write().await.take() {
            if let Err(e) = conn.close().await {
                tracing::error!("Error closing connection: {}", e);
            }
        }
    }
}

use crate::{
    connection::ConnWrapper,
    error::EthStatsError,
    events::{
        AuthMsg, BlockMsg, BlockStats, HistoryMsg, LatencyMsg, NodeInfo, NodeStats, PendingMsg,
        PendingStats, PingMsg, StatsMsg, TxStats, UncleStats,
    },
};
use alloy_consensus::{BlockHeader, Sealable};
use alloy_primitives::U256;
use reth_network_api::{NetworkInfo, Peers};
use reth_primitives_traits::{Block, BlockBody, InMemorySize};
use reth_provider::{
    BlockReader, BlockReaderIdExt, CanonStateNotification, CanonStateSubscriptions,
};
use reth_storage_api::NodePrimitivesProvider;
use reth_transaction_pool::TransactionPool;

use chrono::Local;
use serde_json::Value;
use std::{
    str::FromStr,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    sync::{mpsc, Mutex, RwLock},
    time::{interval, sleep, timeout},
};
use tokio_stream::StreamExt;
use tokio_tungstenite::connect_async;
use url::Url;

/// Number of historical blocks to include in a history update sent to the `EthStats` server
const HISTORY_UPDATE_RANGE: u64 = 50;
/// Duration to wait before attempting to reconnect to the `EthStats` server
const RECONNECT_INTERVAL: Duration = Duration::from_secs(5);
/// Maximum time to wait for a ping response from the server
const PING_TIMEOUT: Duration = Duration::from_secs(5);
/// Interval between regular stats reports to the server
const REPORT_INTERVAL: Duration = Duration::from_secs(15);
/// Maximum time to wait for initial connection establishment
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Maximum time to wait for reading messages from the server
const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Credentials for connecting to an `EthStats` server
///
/// Contains the node identifier, authentication secret, and server host
/// information needed to establish a connection with the `EthStats` service.
#[derive(Debug, Clone)]
pub struct EthstatsCredentials {
    /// Unique identifier for this node in the `EthStats` network
    pub node_id: String,
    /// Authentication secret for the `EthStats` server
    pub secret: String,
    /// Host address of the `EthStats` server
    pub host: String,
}

impl FromStr for EthstatsCredentials {
    type Err = EthStatsError;

    /// Parse credentials from a string in the format "`node_id:secret@host`"
    ///
    /// # Arguments
    /// * `s` - String containing credentials in the format "`node_id:secret@host`"
    ///
    /// # Returns
    /// * `Ok(EthstatsCredentials)` - Successfully parsed credentials
    /// * `Err(EthStatsError::InvalidUrl)` - Invalid format or missing separators
    fn from_str(s: &str) -> Result<Self, Self::Err> {
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

        Ok(Self { node_id, secret, host })
    }
}

/// Main service for interacting with an `EthStats` server
///
/// This service handles all communication with the `EthStats` server including
/// authentication, stats reporting, block notifications, and connection management.
/// It maintains a persistent `WebSocket` connection and automatically reconnects
/// when the connection is lost.
#[derive(Debug)]
pub struct EthStatsService<Network, Provider, Pool> {
    /// Authentication credentials for the `EthStats` server
    pub credentials: EthstatsCredentials,
    /// `WebSocket` connection wrapper, wrapped in Arc<RwLock> for shared access
    pub conn: Arc<RwLock<Option<ConnWrapper>>>,
    /// Timestamp of the last ping sent to the server
    pub last_ping: Arc<Mutex<Option<Instant>>>,
    /// Network interface for getting peer and sync information
    pub network: Network,
    /// Blockchain provider for reading block data and state
    pub provider: Provider,
    /// Transaction pool for getting pending transaction statistics
    pub pool: Pool,
}

impl<Network, Provider, Pool> EthStatsService<Network, Provider, Pool>
where
    Network: NetworkInfo + Peers,
    Provider: BlockReaderIdExt + CanonStateSubscriptions,
    Pool: TransactionPool,
{
    /// Create a new `EthStats` service and establish initial connection
    ///
    /// # Arguments
    /// * `url` - Connection string in format "`node_id:secret@host`"
    /// * `network` - Network interface implementation
    /// * `provider` - Blockchain provider implementation
    /// * `pool` - Transaction pool implementation
    pub async fn new(
        url: &str,
        network: Network,
        provider: Provider,
        pool: Pool,
    ) -> Result<Self, EthStatsError> {
        let credentials = EthstatsCredentials::from_str(url)?;
        let service = Self {
            credentials,
            conn: Arc::new(RwLock::new(None)),
            last_ping: Arc::new(Mutex::new(None)),
            network,
            provider,
            pool,
        };
        service.connect().await?;

        Ok(service)
    }

    /// Establish `WebSocket` connection to the `EthStats` server
    ///
    /// Attempts to connect to the server using the credentials and handles
    /// connection timeouts and errors.
    async fn connect(&self) -> Result<(), EthStatsError> {
        let full_url = format!("ws://{}/api", self.credentials.host);
        let url = Url::parse(&full_url)
            .map_err(|e| EthStatsError::InvalidUrl(format!("Invalid URL: {full_url} - {e}")))?;

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

    /// Authenticate with the `EthStats` server
    ///
    /// Sends authentication credentials and node information to the server
    /// and waits for a successful acknowledgment.
    async fn login(&self) -> Result<(), EthStatsError> {
        let conn = self.conn.read().await;
        let conn = conn.as_ref().ok_or(EthStatsError::NotConnected)?;

        let network_status = self
            .network
            .network_status()
            .await
            .map_err(|e| EthStatsError::AuthError(e.to_string()))?;
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

    /// Report current node statistics to the `EthStats` server
    ///
    /// Sends information about the node's current state including sync status,
    /// peer count, and uptime.
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
                uptime: 100,
            },
        };

        let message = stats_msg.generate_stats_message();
        conn.write_json(&message).await?;

        Ok(())
    }

    /// Send a ping message to the `EthStats` server
    ///
    /// Records the ping time and starts a timeout task to detect if the server
    /// doesn't respond within the expected timeframe.
    async fn send_ping(&self) -> Result<(), EthStatsError> {
        let conn = self.conn.read().await;
        let conn = conn.as_ref().ok_or(EthStatsError::NotConnected)?;

        let ping_time = Instant::now();
        *self.last_ping.lock().await = Some(ping_time);

        let client_time = Local::now().format("%Y-%m-%d %H:%M:%S%.f %:z %Z").to_string();
        let ping_msg = PingMsg { id: self.credentials.node_id.clone(), client_time };

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

    /// Report latency measurement to the `EthStats` server
    ///
    /// Calculates the round-trip time from the last ping and sends it to
    /// the server. This is called when a pong response is received.
    async fn report_latency(&self) -> Result<(), EthStatsError> {
        let mut active = self.last_ping.lock().await;
        if let Some(start) = active.take() {
            let latency = start.elapsed().as_millis() as u64 / 2;

            tracing::debug!("Pong received, latency: {}ms", latency);

            let conn = self.conn.read().await;
            let conn = conn.as_ref().ok_or(EthStatsError::NotConnected)?;

            let latency_msg = LatencyMsg { id: self.credentials.node_id.clone(), latency };

            let message = latency_msg.generate_latency_message();
            conn.write_json(&message).await?
        }

        Ok(())
    }

    /// Report pending transaction count to the `EthStats` server
    ///
    /// Gets the current number of pending transactions from the pool and
    /// sends this information to the server.
    async fn report_pending(&self) -> Result<(), EthStatsError> {
        let conn = self.conn.read().await;
        let conn = conn.as_ref().ok_or(EthStatsError::NotConnected)?;
        let pending = self.pool.pool_size().pending as i32;

        let pending_msg =
            PendingMsg { id: self.credentials.node_id.clone(), stats: PendingStats { pending } };

        let message = pending_msg.generate_pending_message();
        conn.write_json(&message).await?;

        tracing::debug!("Pending handled, pending txs: {}", pending);

        Ok(())
    }

    /// Report block information to the `EthStats` server
    ///
    /// Fetches block data either from a canonical state notification or
    /// the current best block, converts it to stats format, and sends
    /// it to the server.
    ///
    /// # Arguments
    /// * `head` - Optional canonical state notification containing new block info
    async fn report_block(
        &self,
        head: Option<CanonStateNotification<<Provider as NodePrimitivesProvider>::Primitives>>,
    ) -> Result<(), EthStatsError> {
        let conn = self.conn.read().await;
        let conn = conn.as_ref().ok_or(EthStatsError::NotConnected)?;

        let block_number = if let Some(head) = head {
            head.tip().header().number()
        } else {
            self.provider
                .best_block_number()
                .map_err(|e| EthStatsError::DataFetchError(e.to_string()))?
        };

        match self.provider.block_by_id(block_number.into()) {
            Ok(Some(block)) => {
                let block_msg = BlockMsg {
                    id: self.credentials.node_id.clone(),
                    block: self.block_to_stats(&block)?,
                };

                let message = block_msg.generate_block_message();
                conn.write_json(&message).await?;

                tracing::debug!("Report block handled, block_number: {}", block_number);
            }
            Ok(None) => {
                // Block not found, stop fetching
                tracing::warn!("Block {} not found", block_number);
                return Err(EthStatsError::BlockNotFound(block_number));
            }
            Err(e) => {
                tracing::error!("Error fetching block {}: {}", block_number, e);
                return Err(EthStatsError::DataFetchError(e.to_string()));
            }
        };

        Ok(())
    }

    /// Convert a block to `EthStats` block statistics format
    ///
    /// Extracts relevant information from a block and formats it according
    /// to the `EthStats` protocol specification.
    ///
    /// # Arguments
    /// * `block` - The block to convert
    fn block_to_stats(
        &self,
        block: &<Provider as BlockReader>::Block,
    ) -> Result<BlockStats, EthStatsError> {
        let body = block.body();
        let header = block.header();

        let mut txs = vec![];
        for &tx_hash in body.transaction_hashes_iter() {
            txs.push(TxStats { hash: tx_hash })
        }

        Ok(BlockStats {
            number: U256::from(header.number()),
            hash: header.hash_slow(),
            parent_hash: header.parent_hash(),
            timestamp: U256::from(header.timestamp()),
            miner: header.beneficiary(),
            gas_used: header.gas_used(),
            gas_limit: header.gas_limit(),
            diff: header.difficulty().to_string(),
            total_diff: "0".into(),
            txs,
            tx_hash: header.transactions_root(),
            root: header.state_root(),
            uncles: UncleStats(vec![]),
        })
    }

    /// Report historical block data to the `EthStats` server
    ///
    /// Fetches multiple blocks by their numbers and sends their statistics
    /// to the server. This is typically called in response to a history
    /// request from the server.
    ///
    /// # Arguments
    /// * `list` - Vector of block numbers to fetch and report
    async fn report_history(&self, list: Option<&Vec<u64>>) -> Result<(), EthStatsError> {
        let conn = self.conn.read().await;
        let conn = conn.as_ref().ok_or(EthStatsError::NotConnected)?;

        let mut indexes = Vec::with_capacity(HISTORY_UPDATE_RANGE as usize);
        if let Some(list) = list {
            indexes.extend(list);
        } else {
            let best_block_number = self
                .provider
                .best_block_number()
                .map_err(|e| EthStatsError::DataFetchError(e.to_string()))?;

            let start = best_block_number.saturating_sub(HISTORY_UPDATE_RANGE);

            indexes.extend(start..=best_block_number);
        }

        let mut blocks = Vec::with_capacity(indexes.size());
        for block_number in indexes {
            match self.provider.block_by_id(block_number.into()) {
                Ok(Some(block)) => {
                    blocks.push(block);
                }
                Ok(None) => {
                    // Block not found, stop fetching
                    tracing::warn!("Block {} not found", block_number);
                    break;
                }
                Err(e) => {
                    tracing::error!("Error fetching block {}: {}", block_number, e);
                    break;
                }
            }
        }

        let history: Vec<BlockStats> =
            blocks.iter().map(|block| self.block_to_stats(block)).collect::<Result<_, _>>()?;

        if history.is_empty() {
            tracing::warn!("No history to send to stats server");
        } else {
            tracing::debug!(
                "Sending historical blocks to ethstats, first: {}, last: {}",
                history.first().unwrap().number,
                history.last().unwrap().number
            );
        }

        let history_msg = HistoryMsg { id: self.credentials.node_id.clone(), history };

        let message = history_msg.generate_history_message();
        conn.write_json(&message).await?;
        tracing::debug!("History request handled");

        Ok(())
    }

    /// Send a complete status report to the `EthStats` server
    ///
    /// Performs all regular reporting tasks: ping, block info, pending
    /// transactions, and general statistics.
    async fn report(&self) -> Result<(), EthStatsError> {
        self.send_ping().await?;
        self.report_block(None).await?;
        self.report_pending().await?;
        self.report_stats().await?;

        Ok(())
    }

    /// Handle incoming messages from the `EthStats` server
    ///
    /// Parses and processes server messages, dispatching to appropriate
    /// handlers based on the message type.
    async fn handle_message(&self, msg: Value) -> Result<(), EthStatsError> {
        let emit = match msg.get("emit") {
            Some(emit) => emit,
            None => {
                tracing::warn!("Stats server sent non-broadcast, msg {}", msg);
                return Err(EthStatsError::InvalidRequest);
            }
        };

        let command = match emit.get(0) {
            Some(Value::String(command)) => command.as_str(),
            _ => {
                tracing::warn!("Invalid stats server message type, msg {}", msg);
                return Err(EthStatsError::InvalidRequest);
            }
        };

        match command {
            "node-pong" => {
                self.report_latency().await?;
            }
            "history" => {
                let block_numbers = emit
                    .get(1)
                    .and_then(|v| v.as_object())
                    .and_then(|obj| obj.get("list"))
                    .and_then(|v| v.as_array());

                if block_numbers.is_none() {
                    self.report_history(None).await?;

                    return Ok(());
                }

                let block_numbers = block_numbers
                    .unwrap()
                    .iter()
                    .map(|val| {
                        val.as_u64().ok_or_else(|| {
                            tracing::warn!("Invalid stats history block number, msg {}", msg);
                            EthStatsError::InvalidRequest
                        })
                    })
                    .collect::<Result<_, _>>()?;

                self.report_history(Some(&block_numbers)).await?;
            }
            other => tracing::warn!("Unhandled command: {}", other),
        }

        Ok(())
    }

    /// Main service loop that handles all `EthStats` communication
    ///
    /// This method runs the main event loop that:
    /// - Maintains the `WebSocket` connection
    /// - Handles incoming messages from the server
    /// - Reports statistics at regular intervals
    /// - Processes new block notifications
    /// - Automatically reconnects when the connection is lost
    ///
    /// The service runs until explicitly shut down or an unrecoverable
    /// error occurs.
    pub async fn run(self) {
        // Create channels for internal communication
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel(1);
        let (message_tx, mut message_rx) = mpsc::channel(32);
        let (head_tx, mut head_rx) = mpsc::channel(10);

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

        let canonical_stream_handle = {
            let mut canonical_stream = self.provider.canonical_state_stream();
            let head_tx = head_tx.clone();
            let shutdown_tx = shutdown_tx.clone();

            tokio::spawn(async move {
                loop {
                    let head = canonical_stream.next().await;
                    if let Some(head) = head {
                        if head_tx.send(head).await.is_err() {
                            break;
                        }
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

                // Handle new block
                Some(head) = head_rx.recv() => {
                    if let Err(e) = self.report_block(Some(head)).await {
                        tracing::error!("Failed to report block: {}", e);
                        self.disconnect().await;
                    }

                    if let Err(e) = self.report_pending().await {
                        tracing::error!("Failed to report pending: {}", e);
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
        canonical_stream_handle.abort();
    }

    /// Gracefully close the `WebSocket` connection
    ///
    /// Attempts to close the connection cleanly and logs any errors
    /// that occur during the process.
    async fn disconnect(&self) {
        if let Some(conn) = self.conn.write().await.take() {
            if let Err(e) = conn.close().await {
                tracing::error!("Error closing connection: {}", e);
            }
        }
    }
}

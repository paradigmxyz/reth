use crate::{
    connection::ConnWrapper,
    credentials::EthstatsCredentials,
    error::EthStatsError,
    events::{
        AuthMsg, BlockMsg, BlockStats, HistoryMsg, LatencyMsg, NodeInfo, NodeStats, PendingMsg,
        PendingStats, PingMsg, StatsMsg, TxStats, UncleStats,
    },
};
use alloy_consensus::{BlockHeader, Sealable};
use alloy_primitives::U256;
use reth_chain_state::{CanonStateNotification, CanonStateSubscriptions};
use reth_network_api::{NetworkInfo, Peers};
use reth_primitives_traits::{Block, BlockBody};
use reth_storage_api::{BlockReader, BlockReaderIdExt, NodePrimitivesProvider};
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
use tracing::{debug, info};
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

/// Main service for interacting with an `EthStats` server
///
/// This service handles all communication with the `EthStats` server including
/// authentication, stats reporting, block notifications, and connection management.
/// It maintains a persistent `WebSocket` connection and automatically reconnects
/// when the connection is lost.
#[derive(Debug)]
pub struct EthStatsService<Network, Provider, Pool> {
    /// Authentication credentials for the `EthStats` server
    credentials: EthstatsCredentials,
    /// `WebSocket` connection wrapper, wrapped in `Arc<RwLock>` for shared access
    conn: Arc<RwLock<Option<ConnWrapper>>>,
    /// Timestamp of the last ping sent to the server
    last_ping: Arc<Mutex<Option<Instant>>>,
    /// Network interface for getting peer and sync information
    network: Network,
    /// Blockchain provider for reading block data and state
    provider: Provider,
    /// Transaction pool for getting pending transaction statistics
    pool: Pool,
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
        debug!(
            target: "ethstats",
            "Attempting to connect to EthStats server at {}", self.credentials.host
        );
        let full_url = format!("ws://{}/api", self.credentials.host);
        let url = Url::parse(&full_url)
            .map_err(|e| EthStatsError::InvalidUrl(format!("Invalid URL: {full_url} - {e}")))?;

        match timeout(CONNECT_TIMEOUT, connect_async(url.to_string())).await {
            Ok(Ok((ws_stream, _))) => {
                debug!(
                    target: "ethstats",
                    "Successfully connected to EthStats server at {}", self.credentials.host
                );
                let conn: ConnWrapper = ConnWrapper::new(ws_stream);
                *self.conn.write().await = Some(conn.clone());
                self.login().await?;
                Ok(())
            }
            Ok(Err(e)) => Err(EthStatsError::InvalidUrl(e.to_string())),
            Err(_) => {
                debug!(target: "ethstats", "Connection to EthStats server timed out");
                Err(EthStatsError::Timeout)
            }
        }
    }

    /// Authenticate with the `EthStats` server
    ///
    /// Sends authentication credentials and node information to the server
    /// and waits for a successful acknowledgment.
    async fn login(&self) -> Result<(), EthStatsError> {
        debug!(
            target: "ethstats",
            "Attempting to login to EthStats server as node_id {}", self.credentials.node_id
        );
        let conn = self.conn.read().await;
        let conn = conn.as_ref().ok_or(EthStatsError::NotConnected)?;

        let network_status = self
            .network
            .network_status()
            .await
            .map_err(|e| EthStatsError::AuthError(e.to_string()))?;
        let id = &self.credentials.node_id;
        let secret = &self.credentials.secret;
        let protocol = network_status
            .capabilities
            .iter()
            .map(|cap| format!("{}/{}", cap.name, cap.version))
            .collect::<Vec<_>>()
            .join(", ");
        let port = self.network.local_addr().port() as u64;

        let auth = AuthMsg {
            id: id.clone(),
            secret: secret.clone(),
            info: NodeInfo {
                name: id.clone(),
                node: network_status.client_version.clone(),
                port,
                network: self.network.chain_id().to_string(),
                protocol,
                api: "No".to_string(),
                os: std::env::consts::OS.into(),
                os_ver: std::env::consts::ARCH.into(),
                client: "0.1.1".to_string(),
                history: true,
            },
        };

        let message = auth.generate_login_message();
        conn.write_json(&message).await?;

        let response =
            timeout(READ_TIMEOUT, conn.read_json()).await.map_err(|_| EthStatsError::Timeout)??;

        if let Some(ack) = response.get("emit") &&
            ack.get(0) == Some(&Value::String("ready".to_string()))
        {
            info!(
                target: "ethstats",
                "Login successful to EthStats server as node_id {}", self.credentials.node_id
            );
            return Ok(());
        }

        debug!(target: "ethstats", "Login failed: Unauthorized or unexpected login response");
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
                peers: self.network.num_connected_peers() as u64,
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
                debug!(target: "ethstats", "Ping timeout");
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
        let conn = self.conn.read().await;
        let conn = conn.as_ref().ok_or(EthStatsError::NotConnected)?;

        let mut active = self.last_ping.lock().await;
        if let Some(start) = active.take() {
            let latency = start.elapsed().as_millis() as u64 / 2;

            debug!(target: "ethstats", "Reporting latency: {}ms", latency);

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
        let pending = self.pool.pool_size().pending as u64;

        debug!(target: "ethstats", "Reporting pending txs: {}", pending);

        let pending_msg =
            PendingMsg { id: self.credentials.node_id.clone(), stats: PendingStats { pending } };

        let message = pending_msg.generate_pending_message();
        conn.write_json(&message).await?;

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

                debug!(target: "ethstats", "Reporting block: {}", block_number);

                let message = block_msg.generate_block_message();
                conn.write_json(&message).await?;
            }
            Ok(None) => {
                // Block not found, stop fetching
                debug!(target: "ethstats", "Block {} not found", block_number);
                return Err(EthStatsError::BlockNotFound(block_number));
            }
            Err(e) => {
                debug!(target: "ethstats", "Error fetching block {}: {}", block_number, e);
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

        let txs = body.transaction_hashes_iter().copied().map(|hash| TxStats { hash }).collect();

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
            tx_root: header.transactions_root(),
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

        let indexes = if let Some(list) = list {
            list
        } else {
            let best_block_number = self
                .provider
                .best_block_number()
                .map_err(|e| EthStatsError::DataFetchError(e.to_string()))?;

            let start = best_block_number.saturating_sub(HISTORY_UPDATE_RANGE);

            &(start..=best_block_number).collect()
        };

        let mut blocks = Vec::with_capacity(indexes.len());
        for &block_number in indexes {
            match self.provider.block_by_id(block_number.into()) {
                Ok(Some(block)) => {
                    blocks.push(block);
                }
                Ok(None) => {
                    // Block not found, stop fetching
                    debug!(target: "ethstats", "Block {} not found", block_number);
                    break;
                }
                Err(e) => {
                    debug!(target: "ethstats", "Error fetching block {}: {}", block_number, e);
                    break;
                }
            }
        }

        let history: Vec<BlockStats> =
            blocks.iter().map(|block| self.block_to_stats(block)).collect::<Result<_, _>>()?;

        if history.is_empty() {
            debug!(target: "ethstats", "No history to send to stats server");
        } else {
            debug!(
                target: "ethstats",
                "Sending historical blocks to ethstats, first: {}, last: {}",
                history.first().unwrap().number,
                history.last().unwrap().number
            );
        }

        let history_msg = HistoryMsg { id: self.credentials.node_id.clone(), history };

        let message = history_msg.generate_history_message();
        conn.write_json(&message).await?;

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
    /// # Expected Message Variants
    ///
    /// This function expects messages in the following format:
    ///
    /// ```json
    /// { "emit": [<command: String>, <payload: Object>] }
    /// ```
    ///
    /// ## Supported Commands:
    ///
    /// - `"node-pong"`: Indicates a pong response to a previously sent ping. The payload is
    ///   ignored. Triggers a latency report to the server.
    ///   - Example: ```json { "emit": [ "node-pong", { "clientTime": "2025-07-10 12:00:00.123
    ///     +00:00 UTC", "serverTime": "2025-07-10 12:00:01.456 +00:00 UTC" } ] } ```
    ///
    /// - `"history"`: Requests historical block data. The payload may contain a `list` field with
    ///   block numbers to fetch. If `list` is not present, the default range is used.
    ///   - Example with list: `{ "emit": ["history", {"list": [1, 2, 3], "min": 1, "max": 3}] }`
    ///   - Example without list: `{ "emit": ["history", {}] }`
    ///
    /// ## Other Commands:
    ///
    /// Any other command is logged as unhandled and ignored.
    async fn handle_message(&self, msg: Value) -> Result<(), EthStatsError> {
        let emit = match msg.get("emit") {
            Some(emit) => emit,
            None => {
                debug!(target: "ethstats", "Stats server sent non-broadcast, msg {}", msg);
                return Err(EthStatsError::InvalidRequest);
            }
        };

        let command = match emit.get(0) {
            Some(Value::String(command)) => command.as_str(),
            _ => {
                debug!(target: "ethstats", "Invalid stats server message type, msg {}", msg);
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
                            debug!(
                                target: "ethstats",
                                "Invalid stats history block number, msg {}", msg
                            );
                            EthStatsError::InvalidRequest
                        })
                    })
                    .collect::<Result<_, _>>()?;

                self.report_history(Some(&block_numbers)).await?;
            }
            other => debug!(target: "ethstats", "Unhandled command: {}", other),
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
                        match conn.read_json().await {
                            Ok(msg) => {
                                if message_tx.send(msg).await.is_err() {
                                    break;
                                }
                            }
                            Err(e) => {
                                debug!(target: "ethstats", "Read error: {}", e);
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
                    if let Some(head) = head &&
                        head_tx.send(head).await.is_err()
                    {
                        break;
                    }
                }

                let _ = shutdown_tx.send(()).await;
            })
        };

        let mut pending_tx_receiver = self.pool.pending_transactions_listener();

        // Set up intervals
        let mut report_interval = interval(REPORT_INTERVAL);
        let mut reconnect_interval = interval(RECONNECT_INTERVAL);

        // Main event loop using select!
        loop {
            tokio::select! {
                // Handle shutdown signal
                _ = shutdown_rx.recv() => {
                    info!(target: "ethstats", "Shutting down ethstats service");
                    break;
                }

                // Handle messages from the read loop
                Some(msg) = message_rx.recv() => {
                    if let Err(e) = self.handle_message(msg).await {
                        debug!(target: "ethstats", "Error handling message: {}", e);
                        self.disconnect().await;
                    }
                }

                // Handle new block
                Some(head) = head_rx.recv() => {
                    if let Err(e) = self.report_block(Some(head)).await {
                        debug!(target: "ethstats", "Failed to report block: {}", e);
                        self.disconnect().await;
                    }

                    if let Err(e) = self.report_pending().await {
                        debug!(target: "ethstats", "Failed to report pending: {}", e);
                        self.disconnect().await;
                    }
                }

                // Handle new pending tx
                _= pending_tx_receiver.recv() => {
                    if let Err(e) = self.report_pending().await {
                        debug!(target: "ethstats", "Failed to report pending: {}", e);
                        self.disconnect().await;
                    }
                }

                // Handle stats reporting
                _ = report_interval.tick() => {
                    if let Err(e) = self.report().await {
                        debug!(target: "ethstats", "Failed to report: {}", e);
                        self.disconnect().await;
                    }
                }

                // Handle reconnection
                _ = reconnect_interval.tick(), if self.conn.read().await.is_none() => {
                    match self.connect().await {
                        Ok(_) => info!(target: "ethstats", "Reconnected successfully"),
                        Err(e) => debug!(target: "ethstats", "Reconnect failed: {}", e),
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
        if let Some(conn) = self.conn.write().await.take() &&
            let Err(e) = conn.close().await
        {
            debug!(target: "ethstats", "Error closing connection: {}", e);
        }
    }

    /// Test helper to check connection status
    #[cfg(test)]
    pub async fn is_connected(&self) -> bool {
        self.conn.read().await.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use reth_network_api::noop::NoopNetwork;
    use reth_storage_api::noop::NoopProvider;
    use reth_transaction_pool::noop::NoopTransactionPool;
    use serde_json::json;
    use tokio::net::TcpListener;
    use tokio_tungstenite::tungstenite::protocol::{frame::Utf8Bytes, Message};

    const TEST_HOST: &str = "127.0.0.1";
    const TEST_PORT: u16 = 0; // Let OS choose port

    async fn setup_mock_server() -> (String, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind((TEST_HOST, TEST_PORT)).await.unwrap();
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws_stream = tokio_tungstenite::accept_async(stream).await.unwrap();

            // Handle login
            if let Some(Ok(Message::Text(text))) = ws_stream.next().await {
                let value: serde_json::Value = serde_json::from_str(&text).unwrap();
                if value["emit"][0] == "hello" {
                    let response = json!({
                        "emit": ["ready", []]
                    });
                    ws_stream
                        .send(Message::Text(Utf8Bytes::from(response.to_string())))
                        .await
                        .unwrap();
                }
            }

            // Handle ping
            while let Some(Ok(msg)) = ws_stream.next().await {
                if let Message::Text(text) = msg &&
                    text.contains("node-ping")
                {
                    let pong = json!({
                        "emit": ["node-pong", {"id": "test-node"}]
                    });
                    ws_stream.send(Message::Text(Utf8Bytes::from(pong.to_string()))).await.unwrap();
                }
            }
        });

        (addr.to_string(), handle)
    }

    #[tokio::test]
    async fn test_connection_and_login() {
        let (server_url, server_handle) = setup_mock_server().await;
        let ethstats_url = format!("test-node:test-secret@{server_url}");

        let network = NoopNetwork::default();
        let provider = NoopProvider::default();
        let pool = NoopTransactionPool::default();

        let service = EthStatsService::new(&ethstats_url, network, provider, pool)
            .await
            .expect("Service should connect");

        // Verify connection was established
        assert!(service.is_connected().await, "Service should be connected");

        // Clean up server
        server_handle.abort();
    }

    #[tokio::test]
    async fn test_history_command_handling() {
        let (server_url, server_handle) = setup_mock_server().await;
        let ethstats_url = format!("test-node:test-secret@{server_url}");

        let network = NoopNetwork::default();
        let provider = NoopProvider::default();
        let pool = NoopTransactionPool::default();

        let service = EthStatsService::new(&ethstats_url, network, provider, pool)
            .await
            .expect("Service should connect");

        // Simulate receiving a history command
        let history_cmd = json!({
            "emit": ["history", {"list": [1, 2, 3]}]
        });

        service.handle_message(history_cmd).await.expect("History command should be handled");

        // Clean up server
        server_handle.abort();
    }

    #[tokio::test]
    async fn test_invalid_url_handling() {
        let network = NoopNetwork::default();
        let provider = NoopProvider::default();
        let pool = NoopTransactionPool::default();

        // Test missing secret
        let result = EthStatsService::new(
            "test-node@localhost",
            network.clone(),
            provider.clone(),
            pool.clone(),
        )
        .await;
        assert!(
            matches!(result, Err(EthStatsError::InvalidUrl(_))),
            "Should detect invalid URL format"
        );

        // Test invalid URL format
        let result = EthStatsService::new("invalid-url", network, provider, pool).await;
        assert!(
            matches!(result, Err(EthStatsError::InvalidUrl(_))),
            "Should detect invalid URL format"
        );
    }
}

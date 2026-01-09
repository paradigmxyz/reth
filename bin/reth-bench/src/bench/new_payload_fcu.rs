//! Runs the `reth bench` command, calling first newPayload for each block, then calling
//! forkchoiceUpdated.
//!
//! Supports two execution modes:
//! - **Wait-time mode**: If `--wait-time` is provided, uses fixed sleep intervals between blocks
//!   and disables persistence waits.
//! - **Persistence mode**: If `--wait-time` is not provided, waits for every Nth block to be
//!   persisted using the `reth_subscribePersistedBlock` subscription.

use crate::{
    bench::{
        context::BenchContext,
        output::{
            CombinedResult, NewPayloadResult, PersistenceStats, TotalGasOutput, TotalGasRow,
            COMBINED_OUTPUT_SUFFIX, GAS_OUTPUT_SUFFIX, PERSISTENCE_STATS_SUFFIX,
        },
    },
    valid_payload::{block_to_new_payload, call_forkchoice_updated, call_new_payload},
};
use alloy_eips::BlockNumHash;
use alloy_network::Ethereum;
use alloy_provider::{Provider, RootProvider};
use alloy_pubsub::SubscriptionStream;
use alloy_rpc_client::RpcClient;
use alloy_rpc_types_engine::{ForkchoiceState, JwtSecret};
use alloy_transport::Authorization;
use alloy_transport_ws::WsConnect;
use clap::Parser;
use csv::Writer;
use eyre::{Context, OptionExt};
use futures::StreamExt;
use humantime::parse_duration;
use reth_cli_runner::CliContext;
use reth_engine_primitives::config::DEFAULT_PERSISTENCE_THRESHOLD;
use reth_node_core::args::BenchmarkArgs;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};
use url::Url;

/// Persistence threshold: wait for persistence after every (N+1) blocks.
/// This matches the engine's behavior which persists when gap > N.
/// With DEFAULT_PERSISTENCE_THRESHOLD=2, waits at blocks 3, 6, 9, etc.
const PERSISTENCE_THRESHOLD: u64 = DEFAULT_PERSISTENCE_THRESHOLD;

/// Per-checkpoint timeout: wait up to 1 minute for each Nth block to persist
const PERSISTENCE_CHECKPOINT_TIMEOUT: Duration = Duration::from_secs(60);

/// Final sync timeout: wait up to 5 minutes for remaining blocks to persist
const PERSISTENCE_TIMEOUT: Duration = Duration::from_secs(300);

/// Tracks persistence state and statistics during benchmark.
///
/// TODO: Consider simplifying after testing is complete - the core functionality
/// only needs `blocks_sent`, `last_persisted`, and `last_block_number`.
struct PersistenceTracker {
    blocks_sent: u64,
    last_persisted: u64,
    last_block_number: u64,
    wait_count: u64,
    total_wait_time: Duration,
    max_gap: u64,
    gap_sum: u64,
    gap_count: u64,
}

impl PersistenceTracker {
    fn new() -> Self {
        Self {
            blocks_sent: 0,
            last_persisted: 0,
            last_block_number: 0,
            wait_count: 0,
            total_wait_time: Duration::ZERO,
            max_gap: 0,
            gap_sum: 0,
            gap_count: 0,
        }
    }

    fn record_block(&mut self, block_number: u64) {
        self.blocks_sent += 1;
        self.last_block_number = block_number;

        let current_gap = block_number.saturating_sub(self.last_persisted);
        if current_gap > self.max_gap {
            self.max_gap = current_gap;
        }
        self.gap_sum += current_gap;
        self.gap_count += 1;
    }

    fn should_wait(&self) -> bool {
        self.blocks_sent % (PERSISTENCE_THRESHOLD + 1) == 0
    }

    fn record_wait(&mut self, elapsed: Duration, persisted: u64) {
        self.wait_count += 1;
        self.total_wait_time += elapsed;
        self.last_persisted = persisted;
    }

    fn into_stats(self) -> PersistenceStats {
        PersistenceStats {
            wait_count: self.wait_count,
            total_wait_time: self.total_wait_time,
            max_gap: self.max_gap,
            avg_gap: if self.gap_count > 0 {
                self.gap_sum as f64 / self.gap_count as f64
            } else {
                0.0
            },
        }
    }
}

/// Wait for persistence to catch up to target block
async fn wait_for_persistence(
    stream: &mut SubscriptionStream<BlockNumHash>,
    target: u64,
    last_persisted: &mut u64,
    timeout: Duration,
) -> eyre::Result<()> {
    tokio::time::timeout(timeout, async {
        while *last_persisted < target {
            match stream.next().await {
                Some(persisted) => {
                    *last_persisted = persisted.number;
                    debug!(
                        target: "reth-bench",
                        persisted_block = ?last_persisted,
                        "Received persistence notification"
                    );
                }
                None => {
                    return Err(eyre::eyre!("Persistence subscription closed unexpectedly"));
                }
            }
        }
        Ok(())
    })
    .await
    .map_err(|_| {
        eyre::eyre!(
            "Persistence timeout: target block {} not persisted within {:?}. Last persisted: {}",
            target,
            timeout,
            last_persisted
        )
    })?
}

/// `reth benchmark new-payload-fcu` command
#[derive(Debug, Parser)]
pub struct Command {
    /// The RPC url to use for getting data.
    #[arg(long, value_name = "RPC_URL", verbatim_doc_comment)]
    rpc_url: String,

    /// How long to wait after a forkchoice update before sending the next payload.
    /// If provided, uses wait-time mode (fixed waits) and disables persistence-based flow.
    #[arg(long, value_name = "WAIT_TIME", value_parser = parse_duration, verbatim_doc_comment)]
    wait_time: Option<Duration>,

    /// The size of the block buffer (channel capacity) for prefetching blocks from the RPC
    /// endpoint.
    #[arg(
        long = "rpc-block-buffer-size",
        value_name = "RPC_BLOCK_BUFFER_SIZE",
        default_value = "20",
        verbatim_doc_comment
    )]
    rpc_block_buffer_size: usize,

    #[command(flatten)]
    benchmark: BenchmarkArgs,
}

impl Command {
    /// Execute `benchmark new-payload-fcu` command
    pub async fn execute(self, _ctx: CliContext) -> eyre::Result<()> {
        // Determine which mode to use based on wait_time
        let wait_time_mode = match self.wait_time {
            Some(duration) => {
                info!(
                    "Using wait-time mode with {}ms delay (persistence disabled)",
                    duration.as_millis()
                );
                Some(duration)
            }
            None => {
                info!(
                    "Using persistence-based flow (waits after every {} blocks to match engine gap > {} behavior)",
                    PERSISTENCE_THRESHOLD + 1,
                    PERSISTENCE_THRESHOLD
                );
                None
            }
        };

        // Set up persistence subscription stream only if using persistence mode
        let mut persistence_stream = if wait_time_mode.is_none() {
            Some(self.setup_persistence_subscription().await?)
        } else {
            None
        };

        let BenchContext {
            benchmark_mode,
            block_provider,
            auth_provider,
            mut next_block,
            is_optimism,
        } = BenchContext::new(&self.benchmark, self.rpc_url).await?;

        let buffer_size = self.rpc_block_buffer_size;

        // Use a oneshot channel to propagate errors from the spawned task
        let (error_sender, mut error_receiver) = tokio::sync::oneshot::channel();
        let (sender, mut receiver) = tokio::sync::mpsc::channel(buffer_size);

        tokio::task::spawn(async move {
            while benchmark_mode.contains(next_block) {
                let block_res = block_provider
                    .get_block_by_number(next_block.into())
                    .full()
                    .await
                    .wrap_err_with(|| format!("Failed to fetch block by number {next_block}"));
                let block = match block_res.and_then(|opt| opt.ok_or_eyre("Block not found")) {
                    Ok(block) => block,
                    Err(e) => {
                        tracing::error!("Failed to fetch block {next_block}: {e}");
                        let _ = error_sender.send(e);
                        break;
                    }
                };

                let head_block_hash = block.header.hash;
                let safe_block_hash = block_provider
                    .get_block_by_number(block.header.number.saturating_sub(32).into());

                let finalized_block_hash = block_provider
                    .get_block_by_number(block.header.number.saturating_sub(64).into());

                let (safe, finalized) = tokio::join!(safe_block_hash, finalized_block_hash,);

                let safe_block_hash = match safe {
                    Ok(Some(block)) => block.header.hash,
                    Ok(None) | Err(_) => head_block_hash,
                };

                let finalized_block_hash = match finalized {
                    Ok(Some(block)) => block.header.hash,
                    Ok(None) | Err(_) => head_block_hash,
                };

                next_block += 1;
                if let Err(e) = sender
                    .send((block, head_block_hash, safe_block_hash, finalized_block_hash))
                    .await
                {
                    tracing::error!("Failed to send block data: {e}");
                    break;
                }
            }
        });

        let mut tracker = PersistenceTracker::new();
        let mut results = Vec::new();
        let total_benchmark_duration = Instant::now();
        let mut total_wait_time = Duration::ZERO;

        while let Some((block, head, safe, finalized)) = {
            let wait_start = Instant::now();
            let result = receiver.recv().await;
            total_wait_time += wait_start.elapsed();
            result
        } {
            let gas_used = block.header.gas_used;
            let block_number = block.header.number;
            let transaction_count = block.transactions.len() as u64;

            debug!(target: "reth-bench", ?block_number, "Sending payload");

            let forkchoice_state = ForkchoiceState {
                head_block_hash: head,
                safe_block_hash: safe,
                finalized_block_hash: finalized,
            };

            let (version, params) = block_to_new_payload(block, is_optimism)?;
            let start = Instant::now();
            call_new_payload(&auth_provider, version, params).await?;

            let new_payload_result = NewPayloadResult { gas_used, latency: start.elapsed() };

            call_forkchoice_updated(&auth_provider, version, forkchoice_state, None).await?;

            let total_latency = start.elapsed();
            let fcu_latency = total_latency - new_payload_result.latency;
            let combined_result = CombinedResult {
                block_number,
                transaction_count,
                new_payload_result,
                fcu_latency,
                total_latency,
            };

            // Handle waiting based on mode
            if let Some(wait_duration) = wait_time_mode {
                tokio::time::sleep(wait_duration).await;
            } else if let Some(ref mut stream) = persistence_stream {
                tracker.record_block(block_number);

                if tracker.should_wait() {
                    debug!(
                        target: "reth-bench",
                        target_block = ?block_number,
                        last_persisted = ?tracker.last_persisted,
                        blocks_sent = tracker.blocks_sent,
                        "Waiting for persistence"
                    );

                    let wait_start = Instant::now();
                    wait_for_persistence(
                        stream,
                        block_number,
                        &mut tracker.last_persisted,
                        PERSISTENCE_CHECKPOINT_TIMEOUT,
                    )
                    .await?;

                    tracker.record_wait(wait_start.elapsed(), tracker.last_persisted);
                    debug!(target: "reth-bench", elapsed = ?wait_start.elapsed(), "Persistence caught up");
                }
            }

            let current_duration = total_benchmark_duration.elapsed() - total_wait_time;
            info!(%combined_result);

            let gas_row =
                TotalGasRow { block_number, transaction_count, gas_used, time: current_duration };
            results.push((gas_row, combined_result));
        }

        // Check if the spawned task encountered an error
        if let Ok(error) = error_receiver.try_recv() {
            return Err(error);
        }

        // Final sync: wait for remaining blocks (persistence mode only)
        if let Some(ref mut stream) = persistence_stream {
            if tracker.last_persisted < tracker.last_block_number {
                info!(
                    "Waiting for final blocks to persist (current: {}, target: {})",
                    tracker.last_persisted, tracker.last_block_number
                );

                match wait_for_persistence(
                    stream,
                    tracker.last_block_number,
                    &mut tracker.last_persisted,
                    PERSISTENCE_TIMEOUT,
                )
                .await
                {
                    Ok(()) => info!("All blocks persisted successfully"),
                    Err(e) => warn!("Final sync: {}", e),
                }
            }
        }

        let (gas_output_results, combined_results): (_, Vec<CombinedResult>) =
            results.into_iter().unzip();

        let persistence_stats = tracker.into_stats();

        // Write CSV output files
        if let Some(ref path) = self.benchmark.output {
            let output_path = path.join(COMBINED_OUTPUT_SUFFIX);
            info!("Writing engine api call latency output to file: {:?}", output_path);
            let mut writer = Writer::from_path(&output_path)?;
            for result in combined_results {
                writer.serialize(result)?;
            }
            writer.flush()?;

            let output_path = path.join(GAS_OUTPUT_SUFFIX);
            info!("Writing total gas output to file: {:?}", output_path);
            let mut writer = Writer::from_path(&output_path)?;
            for row in &gas_output_results {
                writer.serialize(row)?;
            }
            writer.flush()?;

            if wait_time_mode.is_none() {
                let output_path = path.join(PERSISTENCE_STATS_SUFFIX);
                info!("Writing persistence stats to file: {:?}", output_path);
                let mut writer = Writer::from_path(&output_path)?;
                writer.serialize(&persistence_stats)?;
                writer.flush()?;
            }

            info!("Finished writing benchmark output files to {:?}.", path);
        }

        let gas_output = TotalGasOutput::new(gas_output_results)?;

        if wait_time_mode.is_none() {
            info!(
                total_duration=?gas_output.total_duration,
                total_gas_used=?gas_output.total_gas_used,
                blocks_processed=?gas_output.blocks_processed,
                persistence_waits=?persistence_stats.wait_count,
                persistence_wait_time=?persistence_stats.total_wait_time,
                "Total Ggas/s: {:.4}",
                gas_output.total_gigagas_per_second()
            );
        } else {
            info!(
                total_duration=?gas_output.total_duration,
                total_gas_used=?gas_output.total_gas_used,
                blocks_processed=?gas_output.blocks_processed,
                "Total Ggas/s: {:.4}",
                gas_output.total_gigagas_per_second()
            );
        }

        Ok(())
    }

    /// Gets the WebSocket URL for persistence subscription.
    ///
    /// If --ws-rpc-url is explicitly provided, uses that.
    /// Otherwise, derives from the engine API URL:
    /// - http://localhost:8551 (engine API) → ws://localhost:8546 (RPC WebSocket)
    fn derive_local_rpc_url(&self) -> eyre::Result<Url> {
        let local_url = if let Some(ref ws_url) = self.benchmark.ws_rpc_url {
            // Use explicitly provided WebSocket URL
            let parsed_url: Url = ws_url
                .parse()
                .wrap_err_with(|| format!("Failed to parse WebSocket RPC URL: {ws_url}"))?;

            info!(
                target: "reth-bench",
                ws_url = %parsed_url,
                "Using explicitly provided WebSocket RPC URL"
            );

            parsed_url
        } else {
            // Derive from engine API URL
            let derived_url = engine_url_to_ws_url(&self.benchmark.engine_rpc_url)?;

            debug!(
                target: "reth-bench",
                engine_url = %self.benchmark.engine_rpc_url,
                derived_url = %derived_url,
                "Derived WebSocket URL from engine API URL (use --ws-rpc-url to override)"
            );

            derived_url
        };

        Ok(local_url)
    }

    /// Set up WebSocket subscription for persistence notifications using Alloy pubsub
    async fn setup_persistence_subscription(
        &self,
    ) -> eyre::Result<SubscriptionStream<BlockNumHash>> {
        let auth_jwt = self
            .benchmark
            .auth_jwtsecret
            .clone()
            .ok_or_else(|| eyre::eyre!("--jwt-secret must be provided for persistence mode"))?;

        let jwt_str = std::fs::read_to_string(&auth_jwt)?;
        let jwt = JwtSecret::from_hex(&jwt_str)?;

        // Derive local reth node's WebSocket URL from engine API URL
        let ws_url = self.derive_local_rpc_url()?;

        // Build JWT authorization
        let claims = alloy_rpc_types_engine::Claims::default();
        let token = jwt.encode(&claims)?;

        info!("Connecting to WebSocket at {} for persistence subscription", ws_url);

        // Create WS client with pubsub support
        let ws_connect = WsConnect::new(ws_url.to_string()).with_auth(Authorization::Bearer(token));
        let client = RpcClient::connect_pubsub(ws_connect)
            .await
            .wrap_err("Failed to connect to WebSocket")?;
        let provider: RootProvider<Ethereum> = RootProvider::new(client);

        // Subscribe to persistence notifications
        let subscription = provider
            .subscribe_to::<BlockNumHash>("reth_subscribePersistedBlock")
            .await
            .wrap_err("Failed to subscribe to persistence notifications")?;

        info!("Subscribed to persistence notifications");

        Ok(subscription.into_stream())
    }
}

/// Converts an engine API URL to the corresponding RPC WebSocket URL.
///
/// - Converts http/https to ws/wss
/// - Always uses port 8546 (RPC WebSocket) since the local reth node's WS endpoint
///   is always on this port regardless of what port the engine API uses
fn engine_url_to_ws_url(engine_url: &str) -> eyre::Result<Url> {
    let url: Url = engine_url
        .parse()
        .wrap_err_with(|| format!("Failed to parse engine RPC URL: {engine_url}"))?;

    let mut ws_url = url.clone();

    match ws_url.scheme() {
        "http" => ws_url
            .set_scheme("ws")
            .map_err(|_| eyre::eyre!("Failed to set WS scheme for URL: {url}"))?,
        "https" => ws_url
            .set_scheme("wss")
            .map_err(|_| eyre::eyre!("Failed to set WSS scheme for URL: {url}"))?,
        "ws" | "wss" => {}
        scheme => return Err(eyre::eyre!(
            "Unsupported URL scheme '{scheme}' for URL: {url}. Expected http, https, ws, or wss."
        )),
    }

    // Always use port 8546 for the WS RPC endpoint, regardless of engine API port.
    // The engine API can be on any port (8551, 9551, etc.) but the WS RPC is always 8546.
    ws_url
        .set_port(Some(8546))
        .map_err(|_| eyre::eyre!("Failed to set port for URL: {url}"))?;

    Ok(ws_url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_engine_url_to_ws_url_http_default_port() {
        let result = engine_url_to_ws_url("http://localhost:8551").unwrap();
        assert_eq!(result.scheme(), "ws");
        assert_eq!(result.host_str(), Some("localhost"));
        assert_eq!(result.port(), Some(8546));
    }

    #[test]
    fn test_engine_url_to_ws_url_https() {
        let result = engine_url_to_ws_url("https://localhost:8551").unwrap();
        assert_eq!(result.scheme(), "wss");
        assert_eq!(result.port(), Some(8546));
    }

    #[test]
    fn test_engine_url_to_ws_url_custom_engine_port() {
        // Custom engine API ports (like 9551) should still map to WS port 8546
        let result = engine_url_to_ws_url("http://localhost:9551").unwrap();
        assert_eq!(result.scheme(), "ws");
        assert_eq!(result.port(), Some(8546));
    }

    #[test]
    fn test_engine_url_to_ws_url_already_ws() {
        let result = engine_url_to_ws_url("ws://localhost:8546").unwrap();
        assert_eq!(result.scheme(), "ws");
        assert_eq!(result.port(), Some(8546));
    }

    #[test]
    fn test_engine_url_to_ws_url_invalid_scheme() {
        let result = engine_url_to_ws_url("ftp://localhost:8551");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unsupported URL scheme"));
    }

    #[test]
    fn test_engine_url_to_ws_url_invalid_url() {
        let result = engine_url_to_ws_url("not a valid url");
        assert!(result.is_err());
    }
}

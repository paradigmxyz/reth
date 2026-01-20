//! Runs the `reth bench` command, calling first newPayload for each block, then calling
//! forkchoiceUpdated.
//!
//! Supports configurable waiting behavior:
//! - **`--wait-time`**: Fixed sleep interval between blocks.
//! - **`--wait-for-persistence`**: Waits for every Nth block to be persisted using the
//!   `reth_subscribePersistedBlock` subscription, where N matches the engine's persistence
//!   threshold. This ensures the benchmark doesn't outpace persistence.
//!
//! Both options can be used together or independently.

use crate::{
    bench::{
        context::BenchContext,
        output::{
            write_benchmark_results, CombinedResult, NewPayloadResult, TotalGasOutput, TotalGasRow,
        },
    },
    valid_payload::{block_to_new_payload, call_forkchoice_updated, call_new_payload},
};
use alloy_eips::BlockNumHash;
use alloy_network::Ethereum;
use alloy_provider::{Provider, RootProvider};
use alloy_pubsub::SubscriptionStream;
use alloy_rpc_client::RpcClient;
use alloy_rpc_types_engine::ForkchoiceState;
use alloy_transport_ws::WsConnect;
use clap::Parser;
use eyre::{Context, OptionExt};
use futures::StreamExt;
use humantime::parse_duration;
use reth_cli_runner::CliContext;
use reth_engine_primitives::config::DEFAULT_PERSISTENCE_THRESHOLD;
use reth_node_core::args::BenchmarkArgs;
use std::time::{Duration, Instant};
use tracing::{debug, info};
use url::Url;

const PERSISTENCE_CHECKPOINT_TIMEOUT: Duration = Duration::from_secs(60);

/// `reth benchmark new-payload-fcu` command
#[derive(Debug, Parser)]
pub struct Command {
    /// The RPC url to use for getting data.
    #[arg(long, value_name = "RPC_URL", verbatim_doc_comment)]
    rpc_url: String,

    /// How long to wait after a forkchoice update before sending the next payload.
    #[arg(long, value_name = "WAIT_TIME", value_parser = parse_duration, verbatim_doc_comment)]
    wait_time: Option<Duration>,

    /// Wait for blocks to be persisted before sending the next batch.
    ///
    /// When enabled, waits for every Nth block to be persisted using the
    /// `reth_subscribePersistedBlock` subscription. This ensures the benchmark
    /// doesn't outpace persistence.
    ///
    /// The subscription uses the regular RPC websocket endpoint (no JWT required).
    #[arg(long, default_value = "false", verbatim_doc_comment)]
    wait_for_persistence: bool,

    /// Engine persistence threshold used for deciding when to wait for persistence.
    ///
    /// The benchmark waits after every `(threshold + 1)` blocks. By default this
    /// matches the engine's `DEFAULT_PERSISTENCE_THRESHOLD` (2), so waits occur
    /// at blocks 3, 6, 9, etc.
    #[arg(
        long = "persistence-threshold",
        value_name = "PERSISTENCE_THRESHOLD",
        default_value_t = DEFAULT_PERSISTENCE_THRESHOLD,
        verbatim_doc_comment
    )]
    persistence_threshold: u64,

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
        // Log mode configuration
        if let Some(duration) = self.wait_time {
            info!("Using wait-time mode with {}ms delay between blocks", duration.as_millis());
        }
        if self.wait_for_persistence {
            info!(
                "Persistence waiting enabled (waits after every {} blocks to match engine gap > {} behavior)",
                self.persistence_threshold + 1,
                self.persistence_threshold
            );
        }

        // Set up waiter based on configured options (duration takes precedence)
        let mut waiter = match (self.wait_time, self.wait_for_persistence) {
            (Some(duration), _) => Some(PersistenceWaiter::with_duration(duration)),
            (None, true) => {
                let sub = self.setup_persistence_subscription().await?;
                Some(PersistenceWaiter::with_subscription(
                    sub,
                    self.persistence_threshold,
                    PERSISTENCE_CHECKPOINT_TIMEOUT,
                ))
            }
            (None, false) => None,
        };

        let BenchContext {
            benchmark_mode,
            block_provider,
            auth_provider,
            mut next_block,
            is_optimism,
            ..
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
            let gas_limit = block.header.gas_limit;
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
                gas_limit,
                transaction_count,
                new_payload_result,
                fcu_latency,
                total_latency,
            };

            // Exclude time spent waiting on the block prefetch channel from the benchmark duration.
            // We want to measure engine throughput, not RPC fetch latency.
            let current_duration = total_benchmark_duration.elapsed() - total_wait_time;
            info!(%combined_result);

            if let Some(w) = &mut waiter {
                w.on_block(block_number).await?;
            }

            let gas_row =
                TotalGasRow { block_number, transaction_count, gas_used, time: current_duration };
            results.push((gas_row, combined_result));
        }

        // Check if the spawned task encountered an error
        if let Ok(error) = error_receiver.try_recv() {
            return Err(error);
        }

        // Drop waiter - we don't need to wait for final blocks to persist
        // since the benchmark goal is measuring Ggas/s of newPayload/FCU, not persistence.
        drop(waiter);

        let (gas_output_results, combined_results): (Vec<TotalGasRow>, Vec<CombinedResult>) =
            results.into_iter().unzip();

        if let Some(ref path) = self.benchmark.output {
            write_benchmark_results(path, &gas_output_results, combined_results)?;
        }

        let gas_output = TotalGasOutput::new(gas_output_results)?;

        info!(
            total_duration=?gas_output.total_duration,
            total_gas_used=?gas_output.total_gas_used,
            blocks_processed=?gas_output.blocks_processed,
            "Total Ggas/s: {:.4}",
            gas_output.total_gigagas_per_second()
        );

        Ok(())
    }

    /// Returns the websocket RPC URL used for the persistence subscription.
    ///
    /// Preference:
    /// - If `--ws-rpc-url` is provided, use it directly.
    /// - Otherwise, derive a WS RPC URL from `--engine-rpc-url`.
    ///
    /// The persistence subscription endpoint (`reth_subscribePersistedBlock`) is exposed on
    /// the regular RPC server (WS port, usually 8546), not on the engine API port (usually 8551).
    /// Since `BenchmarkArgs` only has the engine URL by default, we convert the scheme
    /// (http→ws, https→wss) and force the port to 8546.
    fn derive_ws_rpc_url(&self) -> eyre::Result<Url> {
        if let Some(ref ws_url) = self.benchmark.ws_rpc_url {
            let parsed: Url = ws_url
                .parse()
                .wrap_err_with(|| format!("Failed to parse WebSocket RPC URL: {ws_url}"))?;
            info!(target: "reth-bench", ws_url = %parsed, "Using provided WebSocket RPC URL");
            Ok(parsed)
        } else {
            let derived = engine_url_to_ws_url(&self.benchmark.engine_rpc_url)?;
            debug!(
                target: "reth-bench",
                engine_url = %self.benchmark.engine_rpc_url,
                %derived,
                "Derived WebSocket RPC URL from engine RPC URL"
            );
            Ok(derived)
        }
    }

    /// Establishes a websocket connection and subscribes to `reth_subscribePersistedBlock`.
    async fn setup_persistence_subscription(&self) -> eyre::Result<PersistenceSubscription> {
        let ws_url = self.derive_ws_rpc_url()?;

        info!("Connecting to WebSocket at {} for persistence subscription", ws_url);

        let ws_connect = WsConnect::new(ws_url.to_string());
        let client = RpcClient::connect_pubsub(ws_connect)
            .await
            .wrap_err("Failed to connect to WebSocket RPC endpoint")?;
        let provider: RootProvider<Ethereum> = RootProvider::new(client);

        let subscription = provider
            .subscribe_to::<BlockNumHash>("reth_subscribePersistedBlock")
            .await
            .wrap_err("Failed to subscribe to persistence notifications")?;

        info!("Subscribed to persistence notifications");

        Ok(PersistenceSubscription::new(provider, subscription.into_stream()))
    }
}

/// Converts an engine API URL to the default RPC websocket URL.
///
/// Transformations:
/// - `http`  → `ws`
/// - `https` → `wss`
/// - `ws` / `wss` keep their scheme
/// - Port is always set to `8546`, reth's default RPC websocket port.
///
/// This is used when we only know the engine API URL (typically `:8551`) but
/// need to connect to the node's WS RPC endpoint for persistence events.
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
        scheme => {
            return Err(eyre::eyre!(
            "Unsupported URL scheme '{scheme}' for URL: {url}. Expected http, https, ws, or wss."
        ))
        }
    }

    ws_url.set_port(Some(8546)).map_err(|_| eyre::eyre!("Failed to set port for URL: {url}"))?;

    Ok(ws_url)
}

/// Waits until the persistence subscription reports that `target` has been persisted.
///
/// Consumes subscription events until `last_persisted >= target`, or returns an error if:
/// - the subscription stream ends unexpectedly, or
/// - `timeout` elapses before `target` is observed.
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

/// Wrapper that keeps both the subscription stream and the underlying provider alive.
/// The provider must be kept alive for the subscription to continue receiving events.
struct PersistenceSubscription {
    _provider: RootProvider<Ethereum>,
    stream: SubscriptionStream<BlockNumHash>,
}

impl PersistenceSubscription {
    const fn new(
        provider: RootProvider<Ethereum>,
        stream: SubscriptionStream<BlockNumHash>,
    ) -> Self {
        Self { _provider: provider, stream }
    }

    const fn stream_mut(&mut self) -> &mut SubscriptionStream<BlockNumHash> {
        &mut self.stream
    }
}

/// Encapsulates the block waiting logic.
///
/// Provides a simple `on_block()` interface that handles both:
/// - Fixed duration waits (when `wait_time` is set)
/// - Persistence-based waits (when `subscription` is set)
///
/// For persistence mode, waits after every `(threshold + 1)` blocks.
struct PersistenceWaiter {
    wait_time: Option<Duration>,
    subscription: Option<PersistenceSubscription>,
    blocks_sent: u64,
    last_persisted: u64,
    threshold: u64,
    timeout: Duration,
}

impl PersistenceWaiter {
    const fn with_duration(wait_time: Duration) -> Self {
        Self {
            wait_time: Some(wait_time),
            subscription: None,
            blocks_sent: 0,
            last_persisted: 0,
            threshold: 0,
            timeout: Duration::ZERO,
        }
    }

    const fn with_subscription(
        subscription: PersistenceSubscription,
        threshold: u64,
        timeout: Duration,
    ) -> Self {
        Self {
            wait_time: None,
            subscription: Some(subscription),
            blocks_sent: 0,
            last_persisted: 0,
            threshold,
            timeout,
        }
    }

    /// Called once per block. Waits based on the configured mode.
    #[allow(clippy::manual_is_multiple_of)]
    async fn on_block(&mut self, block_number: u64) -> eyre::Result<()> {
        if let Some(wait_time) = self.wait_time {
            tokio::time::sleep(wait_time).await;
            return Ok(());
        }

        let Some(ref mut subscription) = self.subscription else {
            return Ok(());
        };

        self.blocks_sent += 1;

        if self.blocks_sent % (self.threshold + 1) == 0 {
            debug!(
                target: "reth-bench",
                target_block = ?block_number,
                last_persisted = self.last_persisted,
                blocks_sent = self.blocks_sent,
                "Waiting for persistence"
            );

            wait_for_persistence(
                subscription.stream_mut(),
                block_number,
                &mut self.last_persisted,
                self.timeout,
            )
            .await?;

            debug!(
                target: "reth-bench",
                persisted = self.last_persisted,
                "Persistence caught up"
            );
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_engine_url_to_ws_url() {
        // http -> ws, always uses port 8546
        let result = engine_url_to_ws_url("http://localhost:8551").unwrap();
        assert_eq!(result.as_str(), "ws://localhost:8546/");

        // https -> wss
        let result = engine_url_to_ws_url("https://localhost:8551").unwrap();
        assert_eq!(result.as_str(), "wss://localhost:8546/");

        // Custom engine port still maps to 8546
        let result = engine_url_to_ws_url("http://localhost:9551").unwrap();
        assert_eq!(result.port(), Some(8546));

        // Already ws passthrough
        let result = engine_url_to_ws_url("ws://localhost:8546").unwrap();
        assert_eq!(result.scheme(), "ws");

        // Invalid inputs
        assert!(engine_url_to_ws_url("ftp://localhost:8551").is_err());
        assert!(engine_url_to_ws_url("not a valid url").is_err());
    }

    #[tokio::test]
    async fn test_waiter_with_duration() {
        let mut waiter = PersistenceWaiter::with_duration(Duration::from_millis(1));

        let start = Instant::now();
        waiter.on_block(1).await.unwrap();
        waiter.on_block(2).await.unwrap();
        waiter.on_block(3).await.unwrap();

        // Should have waited ~3ms total
        assert!(start.elapsed() >= Duration::from_millis(3));
    }
}

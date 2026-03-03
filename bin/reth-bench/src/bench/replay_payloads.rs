//! Command for replaying pre-generated payloads from disk.
//!
//! This command reads `ExecutionPayloadEnvelopeV4` files from a directory and replays them
//! in sequence using `newPayload` followed by `forkchoiceUpdated`.
//!
//! Supports configurable waiting behavior:
//! - **`--wait-time`**: Fixed sleep interval between blocks.
//! - **`--wait-for-persistence`**: Waits for every Nth block to be persisted using the
//!   `reth_subscribePersistedBlock` subscription, where N matches the engine's persistence
//!   threshold. This ensures the benchmark doesn't outpace persistence.
//!
//! Both options can be used together or independently.

use crate::{
    authenticated_transport::AuthenticatedTransportConnect,
    bench::{
        helpers::parse_duration,
        metrics_scraper::MetricsScraper,
        output::{
            write_benchmark_results, CombinedResult, GasRampPayloadFile, NewPayloadResult,
            TotalGasOutput, TotalGasRow,
        },
        persistence_waiter::{
            derive_ws_rpc_url, setup_persistence_subscription, PersistenceWaiter,
        },
    },
    valid_payload::{call_forkchoice_updated_with_reth, call_new_payload_with_reth},
};
use alloy_primitives::B256;
use alloy_provider::{network::AnyNetwork, Provider, RootProvider};
use alloy_rpc_client::ClientBuilder;
use alloy_rpc_types_engine::{
    CancunPayloadFields, ExecutionData, ExecutionPayloadEnvelopeV4, ExecutionPayloadSidecar,
    ForkchoiceState, JwtSecret, PraguePayloadFields,
};
use clap::Parser;
use eyre::Context;
use reth_cli_runner::CliContext;
use reth_engine_primitives::config::DEFAULT_PERSISTENCE_THRESHOLD;
use reth_node_api::EngineApiMessageVersion;
use reth_rpc_api::RethNewPayloadInput;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use tracing::{debug, info};
use url::Url;

/// `reth bench replay-payloads` command
///
/// Replays pre-generated payloads from a directory by calling `newPayload` followed by
/// `forkchoiceUpdated` for each payload in sequence.
#[derive(Debug, Parser)]
pub struct Command {
    /// The engine RPC URL (with JWT authentication).
    #[arg(long, value_name = "ENGINE_RPC_URL", default_value = "http://localhost:8551")]
    engine_rpc_url: String,

    /// Path to the JWT secret file for engine API authentication.
    #[arg(long, value_name = "JWT_SECRET")]
    jwt_secret: PathBuf,

    /// Directory containing payload files (`payload_block_N.json`).
    #[arg(long, value_name = "PAYLOAD_DIR")]
    payload_dir: PathBuf,

    /// Optional limit on the number of payloads to replay.
    /// If not specified, replays all payloads in the directory.
    #[arg(long, value_name = "COUNT")]
    count: Option<usize>,

    /// Skip the first N payloads.
    #[arg(long, value_name = "SKIP", default_value = "0")]
    skip: usize,

    /// Optional directory containing gas ramp payloads to replay first.
    /// These are replayed before the main payloads to warm up the gas limit.
    #[arg(long, value_name = "GAS_RAMP_DIR")]
    gas_ramp_dir: Option<PathBuf>,

    /// Optional output directory for benchmark results (CSV files).
    #[arg(long, value_name = "OUTPUT")]
    output: Option<PathBuf>,

    /// How long to wait after a forkchoice update before sending the next payload.
    ///
    /// Accepts a duration string (e.g. `100ms`, `2s`) or a bare integer treated as
    /// milliseconds (e.g. `400`).
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

    /// Timeout for waiting on persistence at each checkpoint.
    ///
    /// Must be long enough to account for the persistence thread being blocked
    /// by pruning after the previous save.
    #[arg(
        long = "persistence-timeout",
        value_name = "PERSISTENCE_TIMEOUT",
        value_parser = parse_duration,
        default_value = "120s",
        verbatim_doc_comment
    )]
    persistence_timeout: Duration,

    /// Optional `WebSocket` RPC URL for persistence subscription.
    /// If not provided, derives from engine RPC URL by changing scheme to ws and port to 8546.
    #[arg(long, value_name = "WS_RPC_URL", verbatim_doc_comment)]
    ws_rpc_url: Option<String>,

    /// Use `reth_newPayload` endpoint instead of `engine_newPayload*`.
    ///
    /// The `reth_newPayload` endpoint is a reth-specific extension that takes `ExecutionData`
    /// directly, waits for persistence and cache updates to complete before processing,
    /// and returns server-side timing breakdowns (latency, persistence wait, cache wait).
    #[arg(long, default_value = "false", verbatim_doc_comment)]
    reth_new_payload: bool,

    /// Optional Prometheus metrics endpoint to scrape after each block.
    ///
    /// When provided, reth-bench will fetch metrics from this URL after each
    /// payload, recording per-block execution and state root durations.
    /// Results are written to `metrics.csv` in the output directory.
    #[arg(long = "metrics-url", value_name = "URL", verbatim_doc_comment)]
    metrics_url: Option<String>,
}

/// A loaded payload ready for execution.
struct LoadedPayload {
    /// The index (from filename).
    index: u64,
    /// The payload envelope.
    envelope: ExecutionPayloadEnvelopeV4,
    /// The block hash.
    block_hash: B256,
}

/// A gas ramp payload loaded from disk.
struct GasRampPayload {
    /// Block number from filename.
    block_number: u64,
    /// Engine API version for newPayload.
    ///
    /// `None` indicates that `reth_newPayload` should be used.
    version: Option<EngineApiMessageVersion>,
    /// The file contents.
    file: GasRampPayloadFile,
}

impl Command {
    /// Execute the `replay-payloads` command.
    pub async fn execute(self, _ctx: CliContext) -> eyre::Result<()> {
        info!(target: "reth-bench", payload_dir = %self.payload_dir.display(), "Replaying payloads");

        // Log mode configuration
        if let Some(duration) = self.wait_time {
            info!(target: "reth-bench", "Using wait-time mode with {}ms delay between blocks", duration.as_millis());
        }
        if self.wait_for_persistence {
            info!(
                target: "reth-bench",
                "Persistence waiting enabled (waits after every {} blocks to match engine gap > {} behavior)",
                self.persistence_threshold + 1,
                self.persistence_threshold
            );
        }
        if self.reth_new_payload {
            info!("Using reth_newPayload and reth_forkchoiceUpdated endpoints");
        }

        // Set up waiter based on configured options
        // When both are set: wait at least wait_time, and also wait for persistence if needed
        let mut waiter = match (self.wait_time, self.wait_for_persistence) {
            (Some(duration), true) => {
                let ws_url = derive_ws_rpc_url(self.ws_rpc_url.as_deref(), &self.engine_rpc_url)?;
                let sub = setup_persistence_subscription(ws_url, self.persistence_timeout).await?;
                Some(PersistenceWaiter::with_duration_and_subscription(
                    duration,
                    sub,
                    self.persistence_threshold,
                    self.persistence_timeout,
                ))
            }
            (Some(duration), false) => Some(PersistenceWaiter::with_duration(duration)),
            (None, true) => {
                let ws_url = derive_ws_rpc_url(self.ws_rpc_url.as_deref(), &self.engine_rpc_url)?;
                let sub = setup_persistence_subscription(ws_url, self.persistence_timeout).await?;
                Some(PersistenceWaiter::with_subscription(
                    sub,
                    self.persistence_threshold,
                    self.persistence_timeout,
                ))
            }
            (None, false) => None,
        };

        let mut metrics_scraper = MetricsScraper::maybe_new(self.metrics_url.clone());

        // Set up authenticated engine provider
        let jwt =
            std::fs::read_to_string(&self.jwt_secret).wrap_err("Failed to read JWT secret file")?;
        let jwt = JwtSecret::from_hex(jwt.trim())?;
        let auth_url = Url::parse(&self.engine_rpc_url)?;

        info!(target: "reth-bench", "Connecting to Engine RPC at {}", auth_url);
        let auth_transport = AuthenticatedTransportConnect::new(auth_url.clone(), jwt);
        let auth_client = ClientBuilder::default().connect_with(auth_transport).await?;
        let auth_provider = RootProvider::<AnyNetwork>::new(auth_client);

        // Get parent block (latest canonical block) - we need this for the first FCU
        let parent_block = auth_provider
            .get_block_by_number(alloy_eips::BlockNumberOrTag::Latest)
            .await?
            .ok_or_else(|| eyre::eyre!("Failed to fetch latest block"))?;

        let initial_parent_hash = parent_block.header.hash;
        let initial_parent_number = parent_block.header.number;

        info!(
            target: "reth-bench",
            parent_hash = %initial_parent_hash,
            parent_number = initial_parent_number,
            "Using initial parent block"
        );

        // Load all payloads upfront to avoid I/O delays between phases
        let gas_ramp_payloads = if let Some(ref gas_ramp_dir) = self.gas_ramp_dir {
            let payloads = self.load_gas_ramp_payloads(gas_ramp_dir)?;
            if payloads.is_empty() {
                return Err(eyre::eyre!("No gas ramp payload files found in {:?}", gas_ramp_dir));
            }
            info!(target: "reth-bench", count = payloads.len(), "Loaded gas ramp payloads from disk");
            payloads
        } else {
            Vec::new()
        };

        let payloads = self.load_payloads()?;
        if payloads.is_empty() {
            return Err(eyre::eyre!("No payload files found in {:?}", self.payload_dir));
        }
        info!(target: "reth-bench", count = payloads.len(), "Loaded main payloads from disk");

        let mut parent_hash = initial_parent_hash;

        // Replay gas ramp payloads first
        for (i, payload) in gas_ramp_payloads.iter().enumerate() {
            info!(
                target: "reth-bench",
                gas_ramp_payload = i + 1,
                total = gas_ramp_payloads.len(),
                block_number = payload.block_number,
                block_hash = %payload.file.block_hash,
                "Executing gas ramp payload (newPayload + FCU)"
            );

            let _ = call_new_payload_with_reth(
                &auth_provider,
                payload.version,
                payload.file.params.clone(),
            )
            .await?;

            let fcu_state = ForkchoiceState {
                head_block_hash: payload.file.block_hash,
                safe_block_hash: parent_hash,
                finalized_block_hash: parent_hash,
            };
            call_forkchoice_updated_with_reth(&auth_provider, payload.version, fcu_state).await?;

            info!(target: "reth-bench", gas_ramp_payload = i + 1, "Gas ramp payload executed successfully");

            if let Some(w) = &mut waiter {
                w.on_block(payload.block_number).await?;
            }

            parent_hash = payload.file.block_hash;
        }

        if !gas_ramp_payloads.is_empty() {
            info!(target: "reth-bench", count = gas_ramp_payloads.len(), "All gas ramp payloads replayed");
        }

        let mut results = Vec::new();
        let total_benchmark_duration = Instant::now();

        for (i, payload) in payloads.iter().enumerate() {
            let envelope = &payload.envelope;
            let block_hash = payload.block_hash;
            let execution_payload = &envelope.envelope_inner.execution_payload;
            let inner_payload = &execution_payload.payload_inner.payload_inner;

            let gas_used = inner_payload.gas_used;
            let gas_limit = inner_payload.gas_limit;
            let block_number = inner_payload.block_number;
            let transaction_count =
                execution_payload.payload_inner.payload_inner.transactions.len() as u64;

            debug!(
                target: "reth-bench",
                payload = i + 1,
                total = payloads.len(),
                index = payload.index,
                block_hash = %block_hash,
                "Executing payload (newPayload + FCU)"
            );

            let start = Instant::now();

            debug!(
                target: "reth-bench",
                method = "engine_newPayloadV4",
                block_hash = %block_hash,
                "Sending newPayload"
            );

            let (version, params) = if self.reth_new_payload {
                let reth_data = ExecutionData {
                    payload: execution_payload.clone().into(),
                    sidecar: ExecutionPayloadSidecar::v4(
                        CancunPayloadFields {
                            versioned_hashes: Vec::new(),
                            parent_beacon_block_root: B256::ZERO,
                        },
                        PraguePayloadFields {
                            requests: envelope.execution_requests.clone().into(),
                        },
                    ),
                };
                (None, serde_json::to_value((RethNewPayloadInput::ExecutionData(reth_data),))?)
            } else {
                (
                    Some(EngineApiMessageVersion::V4),
                    serde_json::to_value((
                        execution_payload.clone(),
                        Vec::<B256>::new(),
                        B256::ZERO,
                        envelope.execution_requests.to_vec(),
                    ))?,
                )
            };

            let server_timings =
                call_new_payload_with_reth(&auth_provider, version, params).await?;

            let np_latency =
                server_timings.as_ref().map(|t| t.latency).unwrap_or_else(|| start.elapsed());
            let new_payload_result = NewPayloadResult {
                gas_used,
                latency: np_latency,
                persistence_wait: server_timings.as_ref().and_then(|t| t.persistence_wait),
                execution_cache_wait: server_timings
                    .as_ref()
                    .map(|t| t.execution_cache_wait)
                    .unwrap_or_default(),
                sparse_trie_wait: server_timings
                    .as_ref()
                    .map(|t| t.sparse_trie_wait)
                    .unwrap_or_default(),
            };

            let fcu_state = ForkchoiceState {
                head_block_hash: block_hash,
                safe_block_hash: parent_hash,
                finalized_block_hash: parent_hash,
            };

            let fcu_start = Instant::now();
            call_forkchoice_updated_with_reth(&auth_provider, version, fcu_state).await?;
            let fcu_latency = fcu_start.elapsed();

            let total_latency =
                if server_timings.is_some() { np_latency + fcu_latency } else { start.elapsed() };

            let combined_result = CombinedResult {
                block_number,
                gas_limit,
                transaction_count,
                new_payload_result,
                fcu_latency,
                total_latency,
            };

            let current_duration = total_benchmark_duration.elapsed();
            let progress = format!("{}/{}", i + 1, payloads.len());
            info!(target: "reth-bench", progress, %combined_result);

            if let Some(scraper) = metrics_scraper.as_mut() &&
                let Err(err) = scraper.scrape_after_block(block_number).await
            {
                tracing::warn!(target: "reth-bench", %err, block_number, "Failed to scrape metrics");
            }

            if let Some(w) = &mut waiter {
                w.on_block(block_number).await?;
            }

            let gas_row =
                TotalGasRow { block_number, transaction_count, gas_used, time: current_duration };
            results.push((gas_row, combined_result));

            parent_hash = block_hash;
        }

        // Drop waiter - we don't need to wait for final blocks to persist
        // since the benchmark goal is measuring Ggas/s of newPayload/FCU, not persistence.
        drop(waiter);

        let (gas_output_results, combined_results): (Vec<TotalGasRow>, Vec<CombinedResult>) =
            results.into_iter().unzip();

        if let Some(ref path) = self.output {
            write_benchmark_results(path, &gas_output_results, &combined_results)?;
        }

        if let (Some(path), Some(scraper)) = (&self.output, &metrics_scraper) {
            scraper.write_csv(path)?;
        }

        let gas_output =
            TotalGasOutput::with_combined_results(gas_output_results, &combined_results)?;
        info!(
            target: "reth-bench",
            total_gas_used = gas_output.total_gas_used,
            total_duration = ?gas_output.total_duration,
            execution_duration = ?gas_output.execution_duration,
            blocks_processed = gas_output.blocks_processed,
            wall_clock_ggas_per_second = format_args!("{:.4}", gas_output.total_gigagas_per_second()),
            execution_ggas_per_second = format_args!("{:.4}", gas_output.execution_gigagas_per_second()),
            "Benchmark complete"
        );

        Ok(())
    }

    /// Load and parse all payload files from the directory.
    fn load_payloads(&self) -> eyre::Result<Vec<LoadedPayload>> {
        let mut payloads = Vec::new();

        // Read directory entries
        let entries: Vec<_> = std::fs::read_dir(&self.payload_dir)
            .wrap_err_with(|| format!("Failed to read directory {:?}", self.payload_dir))?
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path().extension().and_then(|s| s.to_str()) == Some("json") &&
                    e.file_name().to_string_lossy().starts_with("payload_block_")
            })
            .collect();

        // Parse filenames to get indices and sort
        let mut indexed_paths: Vec<(u64, PathBuf)> = entries
            .into_iter()
            .filter_map(|e| {
                let name = e.file_name();
                let name_str = name.to_string_lossy();
                // Extract index from "payload_NNN.json"
                let index_str = name_str.strip_prefix("payload_block_")?.strip_suffix(".json")?;
                let index: u64 = index_str.parse().ok()?;
                Some((index, e.path()))
            })
            .collect();

        indexed_paths.sort_by_key(|(idx, _)| *idx);

        // Apply skip and count
        let indexed_paths: Vec<_> = indexed_paths.into_iter().skip(self.skip).collect();
        let indexed_paths: Vec<_> = match self.count {
            Some(count) => indexed_paths.into_iter().take(count).collect(),
            None => indexed_paths,
        };

        // Load each payload
        for (index, path) in indexed_paths {
            let content = std::fs::read_to_string(&path)
                .wrap_err_with(|| format!("Failed to read {:?}", path))?;
            let envelope: ExecutionPayloadEnvelopeV4 = serde_json::from_str(&content)
                .wrap_err_with(|| format!("Failed to parse {:?}", path))?;

            let block_hash =
                envelope.envelope_inner.execution_payload.payload_inner.payload_inner.block_hash;

            debug!(
                target: "reth-bench",
                index = index,
                block_hash = %block_hash,
                path = %path.display(),
                "Loaded payload"
            );

            payloads.push(LoadedPayload { index, envelope, block_hash });
        }

        Ok(payloads)
    }

    /// Load and parse gas ramp payload files from a directory.
    fn load_gas_ramp_payloads(&self, dir: &PathBuf) -> eyre::Result<Vec<GasRampPayload>> {
        let mut payloads = Vec::new();

        let entries: Vec<_> = std::fs::read_dir(dir)
            .wrap_err_with(|| format!("Failed to read directory {:?}", dir))?
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path().extension().and_then(|s| s.to_str()) == Some("json") &&
                    e.file_name().to_string_lossy().starts_with("payload_block_")
            })
            .collect();

        // Parse filenames to get block numbers and sort
        let mut indexed_paths: Vec<(u64, PathBuf)> = entries
            .into_iter()
            .filter_map(|e| {
                let name = e.file_name();
                let name_str = name.to_string_lossy();
                // Extract block number from "payload_block_NNN.json"
                let block_str = name_str.strip_prefix("payload_block_")?.strip_suffix(".json")?;
                let block_number: u64 = block_str.parse().ok()?;
                Some((block_number, e.path()))
            })
            .collect();

        indexed_paths.sort_by_key(|(num, _)| *num);

        for (block_number, path) in indexed_paths {
            let content = std::fs::read_to_string(&path)
                .wrap_err_with(|| format!("Failed to read {:?}", path))?;
            let file: GasRampPayloadFile = serde_json::from_str(&content)
                .wrap_err_with(|| format!("Failed to parse {:?}", path))?;

            let version = if let Some(version) = file.version {
                match version {
                    1 => EngineApiMessageVersion::V1,
                    2 => EngineApiMessageVersion::V2,
                    3 => EngineApiMessageVersion::V3,
                    4 => EngineApiMessageVersion::V4,
                    5 => EngineApiMessageVersion::V5,
                    v => return Err(eyre::eyre!("Invalid version {} in {:?}", v, path)),
                }
                .into()
            } else {
                None
            };

            info!(
                block_number,
                block_hash = %file.block_hash,
                path = %path.display(),
                "Loaded gas ramp payload"
            );

            payloads.push(GasRampPayload { block_number, version, file });
        }

        Ok(payloads)
    }
}

//! Command for replaying pre-generated payloads from disk.

use crate::{
    authenticated_transport::AuthenticatedTransportConnect,
    bench::{
        generate_big_block::{compute_payload_block_hash, BigBlockPayload},
        helpers::parse_duration,
        metrics_scraper::MetricsScraper,
        output::{
            write_benchmark_results, CombinedResult, NewPayloadResult, TotalGasOutput, TotalGasRow,
        },
    },
    valid_payload::{call_forkchoice_updated_with_reth, call_new_payload_with_reth},
};
use alloy_eip7928::bal::Bal;
use alloy_eips::eip7928::BlockAccessList;
use alloy_primitives::B256;
use alloy_provider::{network::AnyNetwork, Provider, RootProvider};
use alloy_rpc_client::ClientBuilder;
use alloy_rpc_types_engine::{
    CancunPayloadFields, ExecutionData, ExecutionPayload, ExecutionPayloadEnvelopeV4,
    ExecutionPayloadSidecar, ExecutionPayloadV4, ForkchoiceState, JwtSecret, PraguePayloadFields,
};
use clap::Parser;
use eyre::Context;
use reth_cli_runner::CliContext;
use reth_engine_primitives::BigBlockData;
use reth_node_api::EngineApiMessageVersion;
use reth_node_core::args::WaitForPersistence;
use reth_rpc_api::RethNewPayloadInput;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use tracing::{debug, info, warn};
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

    /// Deprecated: gas ramp is no longer needed. This flag is accepted but ignored.
    #[arg(long, value_name = "GAS_RAMP_DIR", hide = true)]
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

    /// Use `reth_newPayload` endpoint instead of `engine_newPayload*`.
    ///
    /// The `reth_newPayload` endpoint is a reth-specific extension that takes `ExecutionData`
    /// directly, waits for persistence and cache updates to complete before processing,
    /// and returns server-side timing breakdowns (latency, persistence wait, cache wait).
    #[arg(long, default_value = "false", verbatim_doc_comment)]
    reth_new_payload: bool,

    /// Forward embedded block access lists to `reth_newPayload` when payload files contain them.
    ///
    /// Disabled by default so the same payload set can be replayed with or without BALs.
    ///
    /// Requires `--reth-new-payload`.
    #[arg(long, default_value = "false", verbatim_doc_comment, requires = "reth_new_payload")]
    bal: bool,

    /// Control when `reth_newPayload` waits for in-flight persistence.
    ///
    /// Accepts `always` (default — wait on every block), `never`, or a number N
    /// to wait every N blocks and skip the rest.
    ///
    /// Requires `--reth-new-payload`.
    #[arg(
        long = "wait-for-persistence",
        value_name = "MODE",
        num_args = 0..=1,
        default_missing_value = "always",
        value_parser = clap::value_parser!(WaitForPersistence),
        requires = "reth_new_payload",
        verbatim_doc_comment
    )]
    wait_for_persistence: Option<WaitForPersistence>,

    /// Skip waiting for execution cache and sparse trie locks before processing.
    ///
    /// Only works with `--reth-new-payload`. When set, passes `wait_for_caches: false`
    /// to the `reth_newPayload` endpoint.
    #[arg(long, default_value = "false", verbatim_doc_comment, requires = "reth_new_payload")]
    no_wait_for_caches: bool,

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
    /// The execution data for the block.
    execution_data: ExecutionData,
    /// The block hash.
    block_hash: B256,
    /// Big block data containing environment switches and prior block hashes.
    big_block_data: BigBlockData<ExecutionData>,
    /// Optional BAL flattened into the payload file.
    block_access_list: Option<BlockAccessList>,
}

impl Command {
    /// Execute the `replay-payloads` command.
    pub async fn execute(self, _ctx: CliContext) -> eyre::Result<()> {
        info!(target: "reth-bench", payload_dir = %self.payload_dir.display(), "Replaying payloads");

        // Log mode configuration
        if let Some(duration) = self.wait_time {
            info!(target: "reth-bench", "Using wait-time mode with {}ms minimum interval between blocks", duration.as_millis());
        }
        if self.reth_new_payload {
            info!("Using reth_newPayload and reth_forkchoiceUpdated endpoints");
            if self.bal {
                info!(target: "reth-bench", "Forwarding embedded block_access_list data");
            }
        }

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

        // Warn if deprecated --gas-ramp-dir is passed
        if self.gas_ramp_dir.is_some() {
            warn!(
                target: "reth-bench",
                "--gas-ramp-dir is deprecated and ignored."
            );
        }

        // Load all payloads upfront to avoid I/O delays between phases
        let payloads = self.load_payloads()?;
        if payloads.is_empty() {
            return Err(eyre::eyre!("No payload files found in {:?}", self.payload_dir));
        }
        info!(target: "reth-bench", count = payloads.len(), "Loaded main payloads from disk");

        let has_env_switches = payloads.iter().any(|p| !p.big_block_data.env_switches.is_empty());
        let has_block_access_lists = payloads.iter().any(|p| {
            p.block_access_list.as_ref().is_some_and(|bal: &BlockAccessList| !bal.is_empty())
        });

        // If any payload has env_switches but we're not using reth_newPayload, warn the user
        if !self.reth_new_payload {
            if has_env_switches {
                warn!(
                    target: "reth-bench",
                    "Payloads contain env_switches but --reth-new-payload is not set. \
                     env_switches are only supported with reth_newPayload and will be ignored."
                );
            }
            if has_block_access_lists {
                warn!(
                    target: "reth-bench",
                    "Payloads contain block_access_list data but --reth-new-payload is not set. \
                     BALs are only forwarded with reth_newPayload and will be ignored."
                );
            }
        } else if has_block_access_lists && !self.bal {
            info!(
                target: "reth-bench",
                "Payloads contain block_access_list data but --bal is not set. BALs will be ignored."
            );
        }

        let mut parent_hash = initial_parent_hash;

        let mut results = Vec::new();
        let total_benchmark_duration = Instant::now();

        for (i, payload) in payloads.iter().enumerate() {
            let execution_data = &payload.execution_data;
            let mut block_hash = payload.block_hash;
            let v1 = execution_data.payload.as_v1();

            let gas_used = v1.gas_used;
            let gas_limit = v1.gas_limit;
            let block_number = v1.block_number;
            let transaction_count = v1.transactions.len() as u64;

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
                let big_block_data_param = if payload.big_block_data.env_switches.is_empty() &&
                    payload.big_block_data.prior_block_hashes.is_empty()
                {
                    None
                } else {
                    Some(payload.big_block_data.clone())
                };
                let wait_for_persistence = self
                    .wait_for_persistence
                    .unwrap_or(WaitForPersistence::Never)
                    .rpc_value(block_number);

                // Inject sidecar BAL into the inline V4 payload field when --bal is set.
                // If the payload is not already V4 we upgrade it (V3→V4) so the BAL
                // can be carried inline. This changes the block hash, so we recompute
                // it and patch parent_hash to maintain the chain.
                let mut execution_data = execution_data.clone();
                if self.bal &&
                    let Some(bal) = &payload.block_access_list
                {
                    let encoded_bal: alloy_primitives::Bytes =
                        alloy_rlp::encode(Bal::from(bal.clone())).into();

                    // Upgrade to V4 if necessary, then set the BAL field.
                    if execution_data.payload.as_v4().is_none() {
                        execution_data.payload = upgrade_to_v4(execution_data.payload, encoded_bal);
                    } else {
                        execution_data.payload.as_v4_mut().unwrap().block_access_list = encoded_bal;
                    }

                    // Patch parent_hash so this block chains off the (possibly
                    // rehashed) previous block.
                    execution_data.payload.as_v1_mut().parent_hash = parent_hash;

                    // Recompute block hash after payload modification and update
                    // the hash stored in the payload itself.
                    block_hash = compute_payload_block_hash(&execution_data)?;
                    execution_data.payload.as_v1_mut().block_hash = block_hash;
                }

                (
                    None,
                    serde_json::to_value((
                        RethNewPayloadInput::ExecutionData(execution_data),
                        wait_for_persistence,
                        self.no_wait_for_caches.then_some(false),
                        big_block_data_param,
                    ))?,
                )
            } else {
                let requests =
                    execution_data.sidecar.requests().cloned().unwrap_or_default().to_vec();
                (
                    Some(EngineApiMessageVersion::V4),
                    serde_json::to_value((
                        execution_data.payload.clone(),
                        Vec::<B256>::new(),
                        B256::ZERO,
                        requests,
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
                persistence_wait: server_timings
                    .as_ref()
                    .map(|t| t.persistence_wait)
                    .unwrap_or_default(),
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

            if let Some(wait_time) = self.wait_time {
                let remaining = wait_time.saturating_sub(start.elapsed());
                if !remaining.is_zero() {
                    tokio::time::sleep(remaining).await;
                }
            }

            let gas_row =
                TotalGasRow { block_number, transaction_count, gas_used, time: current_duration };
            results.push((gas_row, combined_result));

            parent_hash = block_hash;
        }

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
    ///
    /// Tries to load each file as a [`BigBlockPayload`] first (which includes `env_switches`),
    /// falling back to [`ExecutionPayloadEnvelopeV4`] for backwards compatibility.
    fn load_payloads(&self) -> eyre::Result<Vec<LoadedPayload>> {
        let mut payloads = Vec::new();

        // Read directory entries — match both legacy "payload_block_*.json" and new
        // "big_block_*.json" formats
        let entries: Vec<_> = std::fs::read_dir(&self.payload_dir)
            .wrap_err_with(|| format!("Failed to read directory {:?}", self.payload_dir))?
            .filter_map(|e| e.ok())
            .filter(|e| {
                let name = e.file_name();
                let name_str = name.to_string_lossy();
                e.path().extension().and_then(|s| s.to_str()) == Some("json") &&
                    (name_str.starts_with("payload_block_") ||
                        name_str.starts_with("big_block_"))
            })
            .collect();

        // Parse filenames to get indices and sort.
        // Supports "payload_block_N.json" and "big_block_FROM_to_TO.json" naming.
        let mut indexed_paths: Vec<(u64, PathBuf)> = entries
            .into_iter()
            .filter_map(|e| {
                let name = e.file_name();
                let name_str = name.to_string_lossy();
                let index = if let Some(rest) = name_str.strip_prefix("payload_block_") {
                    rest.strip_suffix(".json")?.parse::<u64>().ok()?
                } else if let Some(rest) = name_str.strip_prefix("big_block_") {
                    // "big_block_FROM_to_TO.json" — use FROM as the index
                    let rest = rest.strip_suffix(".json")?;
                    rest.split("_to_").next()?.parse::<u64>().ok()?
                } else {
                    return None;
                };
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

            // Try BigBlockPayload first, then fall back to legacy ExecutionPayloadEnvelopeV4
            let (execution_data, big_block_data, block_access_list) = if let Ok(big_block) =
                serde_json::from_str::<BigBlockPayload>(&content)
            {
                (big_block.execution_data, big_block.big_block_data, big_block.block_access_list)
            } else {
                let envelope: ExecutionPayloadEnvelopeV4 = serde_json::from_str(&content)
                    .wrap_err_with(|| format!("Failed to parse {:?}", path))?;
                let execution_data = ExecutionData {
                    payload: envelope.envelope_inner.execution_payload.clone().into(),
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
                (execution_data, BigBlockData::default(), None)
            };

            let block_hash = execution_data.payload.as_v1().block_hash;

            debug!(
                target: "reth-bench",
                index = index,
                block_hash = %block_hash,
                env_switches = big_block_data.env_switches.len(),
                prior_block_hashes = big_block_data.prior_block_hashes.len(),
                bal_accounts = block_access_list.as_ref().map_or(0, Vec::len),
                path = %path.display(),
                "Loaded payload"
            );

            payloads.push(LoadedPayload {
                index,
                execution_data,
                block_hash,
                big_block_data,
                block_access_list,
            });
        }

        Ok(payloads)
    }
}

/// Upgrades an [`ExecutionPayload`] to V4 by wrapping the inner V3 payload (constructing
/// default V2/V3 layers for V1 payloads if needed) and setting the provided BAL bytes.
fn upgrade_to_v4(
    payload: ExecutionPayload,
    block_access_list: alloy_primitives::Bytes,
) -> ExecutionPayload {
    use alloy_rpc_types_engine::{ExecutionPayloadV2, ExecutionPayloadV3};

    let v3 = match payload {
        ExecutionPayload::V4(_) => unreachable!("caller checks as_v4().is_none()"),
        ExecutionPayload::V3(v3) => v3,
        ExecutionPayload::V2(v2) => {
            ExecutionPayloadV3 { payload_inner: v2, blob_gas_used: 0, excess_blob_gas: 0 }
        }
        ExecutionPayload::V1(v1) => ExecutionPayloadV3 {
            payload_inner: ExecutionPayloadV2 { payload_inner: v1, withdrawals: Vec::new() },
            blob_gas_used: 0,
            excess_blob_gas: 0,
        },
    };

    ExecutionPayload::V4(ExecutionPayloadV4 {
        payload_inner: v3,
        block_access_list,
        slot_number: 0,
    })
}

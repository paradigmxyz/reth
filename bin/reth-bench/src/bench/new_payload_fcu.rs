//! Runs the `reth bench` command, calling first newPayload for each block, then calling
//! forkchoiceUpdated.

use crate::{
    bench::{
        context::BenchContext,
        helpers::{fetch_block_access_list, parse_duration},
        metrics_scraper::MetricsScraper,
        output::{
            write_benchmark_results, CombinedResult, NewPayloadResult, TotalGasOutput, TotalGasRow,
        },
    },
    valid_payload::{
        block_to_new_payload, call_forkchoice_updated_with_reth, call_new_payload_with_reth,
        payload_to_new_payload,
    },
};
use alloy_consensus::TxEnvelope;
use alloy_eips::Encodable2718;
use alloy_primitives::{Bytes, B256};
use alloy_provider::{
    ext::DebugApi,
    network::{AnyNetwork, AnyRpcBlock},
    Provider, RootProvider,
};
use alloy_rpc_types_engine::{
    ExecutionData, ExecutionPayload, ExecutionPayloadEnvelopeV5, ExecutionPayloadInputV2,
    ExecutionPayloadSidecar, ForkchoiceState, PayloadAttributes,
};
use clap::Parser;
use eyre::{bail, Context, OptionExt};
use futures::{stream, StreamExt, TryStreamExt};
use reth_cli_runner::CliContext;
use reth_engine_primitives::config::DEFAULT_PERSISTENCE_THRESHOLD;
use reth_node_api::EngineApiMessageVersion;
use reth_node_core::args::{BenchmarkArgs, WaitForPersistence};
use reth_rpc_api::{RethNewPayloadInput, TestingBuildBlockRequestV1};
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

/// `reth benchmark new-payload-fcu` command
#[derive(Debug, Parser)]
pub struct Command {
    /// The RPC url to use for getting data.
    #[arg(long, value_name = "RPC_URL", verbatim_doc_comment)]
    rpc_url: String,

    /// Build a separate fork with `testing_buildBlockV1` and alternate forkchoice updates between
    /// the canonical chain and that fork on every block while the fork grows up to the configured
    /// depth.
    ///
    /// This requires enabling the hidden `testing` RPC module on the target node,
    /// for example with `reth node --http --http.api eth,testing`.
    ///
    /// Passing `--reorg` with no value uses a depth of `8`.
    #[arg(
        long,
        value_name = "DEPTH",
        num_args = 0..=1,
        default_missing_value = "8",
        value_parser = parse_reorg_depth,
        verbatim_doc_comment
    )]
    reorg: Option<usize>,

    /// How long to wait after a forkchoice update before sending the next payload.
    ///
    /// Accepts a duration string (e.g. `100ms`, `2s`) or a bare integer treated as
    /// milliseconds (e.g. `400`).
    #[arg(long, value_name = "WAIT_TIME", value_parser = parse_duration, verbatim_doc_comment)]
    wait_time: Option<Duration>,

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

    /// The size of the block buffer (channel capacity) for prefetching blocks from the RPC
    /// endpoint.
    #[arg(
        long = "rpc-block-buffer-size",
        value_name = "RPC_BLOCK_BUFFER_SIZE",
        default_value = "20",
        verbatim_doc_comment
    )]
    rpc_block_buffer_size: usize,

    /// Weather to enable bal by default or not.
    #[arg(long, default_value = "false", verbatim_doc_comment)]
    enable_bal: bool,

    #[command(flatten)]
    benchmark: BenchmarkArgs,
}

#[derive(Debug)]
struct PreparedBuiltBlock {
    block_hash: B256,
    version: Option<EngineApiMessageVersion>,
    params: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActiveBranch {
    Canonical,
    Fork,
}

#[derive(Debug)]
struct ReorgState {
    depth: usize,
    active_branch: ActiveBranch,
    fork_length: usize,
    branch_point_hash: Option<B256>,
    fork_parent_hash: Option<B256>,
}

impl ReorgState {
    const fn new(depth: usize) -> Self {
        Self {
            depth,
            active_branch: ActiveBranch::Canonical,
            fork_length: 0,
            branch_point_hash: None,
            fork_parent_hash: None,
        }
    }

    fn next_fork_parent_hash(&self, canonical_parent_hash: B256) -> B256 {
        self.fork_parent_hash.unwrap_or(canonical_parent_hash)
    }

    const fn next_target_branch(&self) -> ActiveBranch {
        match self.active_branch {
            ActiveBranch::Canonical => ActiveBranch::Fork,
            ActiveBranch::Fork => ActiveBranch::Canonical,
        }
    }

    const fn push_fork_head(&mut self, canonical_parent_hash: B256, fork_head_hash: B256) {
        if self.fork_length == 0 {
            self.branch_point_hash = Some(canonical_parent_hash);
        }
        self.fork_length += 1;
        self.fork_parent_hash = Some(fork_head_hash);
    }

    fn forkchoice_state(&self, fork_head_hash: B256) -> eyre::Result<ForkchoiceState> {
        let branch_point_hash = self.branch_point_hash.ok_or_eyre("missing reorg branch point")?;

        Ok(ForkchoiceState {
            head_block_hash: fork_head_hash,
            safe_block_hash: branch_point_hash,
            finalized_block_hash: branch_point_hash,
        })
    }

    const fn reset(&mut self) {
        self.fork_length = 0;
        self.branch_point_hash = None;
        self.fork_parent_hash = None;
        self.active_branch = ActiveBranch::Canonical;
    }
}

impl Command {
    /// Execute `benchmark new-payload-fcu` command
    pub async fn execute(self, _ctx: CliContext) -> eyre::Result<()> {
        if self.reorg.is_some() && self.benchmark.rlp_blocks {
            bail!("--reorg cannot be combined with --rlp-blocks")
        }
        if self.reorg.is_some() && self.enable_bal {
            bail!("--reorg cannot be combined with --enable-bal")
        }

        // Log mode configuration
        if let Some(duration) = self.wait_time {
            info!(target: "reth-bench", "Using wait-time mode with {}ms minimum interval between blocks", duration.as_millis());
        }
        if let Some(depth) = self.reorg {
            info!(target: "reth-bench", depth, "Using testing_buildBlockV1 reorg mode");
        }

        let BenchContext {
            benchmark_mode,
            block_provider,
            auth_provider,
            local_rpc_provider,
            next_block,
            use_reth_namespace,
            rlp_blocks,
            wait_for_persistence,
            no_wait_for_caches,
        } = BenchContext::new(&self.benchmark, self.rpc_url).await?;

        let total_blocks = benchmark_mode.total_blocks();

        let mut metrics_scraper = MetricsScraper::maybe_new(self.benchmark.metrics_url.clone());

        if use_reth_namespace {
            info!("Using reth_newPayload and reth_forkchoiceUpdated endpoints");
        }

        let buffer_size = self.rpc_block_buffer_size;

        let mut blocks = Box::pin(
            stream::iter((next_block..)
                .take_while(|next_block| {
                    benchmark_mode.contains(*next_block)
                }))
                .map(|next_block| {
                    let block_provider = block_provider.clone();
                    async move {
                        let block_res = block_provider
                            .get_block_by_number(next_block.into())
                            .full()
                            .await
                            .wrap_err_with(|| {
                                format!("Failed to fetch block by number {next_block}")
                            });
                        let block =
                            match block_res.and_then(|opt| opt.ok_or_eyre("Block not found")) {
                                Ok(block) => block,
                                Err(e) => {
                                    tracing::error!(target: "reth-bench", "Failed to fetch block {next_block}: {e}");
                                    return Err(e)
                                }
                            };

                        let rlp = if rlp_blocks {
                            let rlp = match block_provider
                                .debug_get_raw_block(next_block.into())
                                .await
                            {
                                Ok(rlp) => rlp,
                                Err(e) => {
                                    tracing::error!(target: "reth-bench", "Failed to fetch raw block {next_block}: {e}");
                                    return Err(e.into())
                                }
                            };
                            Some(rlp)
                        } else {
                            None
                        };

                        let head_block_hash = block.header.hash;
                        let safe_block_hash = block_provider
                            .get_block_by_number(block.header.number.saturating_sub(32).into());

                        let finalized_block_hash = block_provider
                            .get_block_by_number(block.header.number.saturating_sub(64).into());

                        let (safe, finalized) =
                            tokio::join!(safe_block_hash, finalized_block_hash);

                        let safe_block_hash = match safe {
                            Ok(Some(block)) => block.header.hash,
                            Ok(None) | Err(_) => head_block_hash,
                        };

                        let finalized_block_hash = match finalized {
                            Ok(Some(block)) => block.header.hash,
                            Ok(None) | Err(_) => head_block_hash,
                        };

                        Ok((block, head_block_hash, safe_block_hash, finalized_block_hash, rlp))
                    }
                })
                .buffered(buffer_size),
        );

        let mut results = Vec::new();
        let mut blocks_processed = 0u64;
        let total_benchmark_duration = Instant::now();
        let mut total_wait_time = Duration::ZERO;
        let mut reorg_state = self.reorg.map(ReorgState::new);
        while let Some((block, head, safe, finalized, rlp)) = {
            let wait_start = Instant::now();
            let result = blocks.try_next().await?;
            total_wait_time += wait_start.elapsed();
            result
        } {
            let gas_used = block.header.gas_used;
            let gas_limit = block.header.gas_limit;
            let block_number = block.header.number;
            let canonical_parent_hash = block.header.parent_hash;
            let transaction_count = block.transactions.len() as u64;
            let canonical_forkchoice_state = ForkchoiceState {
                head_block_hash: head,
                safe_block_hash: safe,
                finalized_block_hash: finalized,
            };

            let next_target_branch = reorg_state.as_ref().map(ReorgState::next_target_branch);

            let built_fork_block = if let (Some(reorg_state), Some(ActiveBranch::Fork)) =
                (reorg_state.as_ref(), next_target_branch)
            {
                Some(
                    prepare_built_block(
                        &local_rpc_provider,
                        &block,
                        reorg_state.next_fork_parent_hash(canonical_parent_hash),
                        use_reth_namespace,
                        wait_for_persistence,
                        no_wait_for_caches,
                    )
                    .await?,
                )
            } else {
                None
            };

            let bal = if rlp.is_none() &&
                (block.header.block_access_list_hash.is_some() || self.enable_bal)
            {
                Some(fetch_block_access_list(&block_provider, block.header.number).await?)
            } else {
                None
            };

            let (version, params) = block_to_new_payload(
                block,
                rlp,
                use_reth_namespace,
                wait_for_persistence,
                no_wait_for_caches,
                bal,
            )?;

            debug!(target: "reth-bench", ?block_number, "Sending payload");
            let mut forkchoice_state = canonical_forkchoice_state;
            let mut forkchoice_version = version;

            if let Some(prepared) = built_fork_block {
                call_new_payload_with_reth(&auth_provider, prepared.version, prepared.params)
                    .await?;

                if let Some(reorg_state) = reorg_state.as_mut() {
                    reorg_state.push_fork_head(canonical_parent_hash, prepared.block_hash);
                    forkchoice_state = reorg_state.forkchoice_state(prepared.block_hash)?;
                    forkchoice_version = prepared.version;
                    reorg_state.active_branch = ActiveBranch::Fork;

                    info!(
                        target: "reth-bench",
                        block_number,
                        branch_point = %forkchoice_state.safe_block_hash,
                        fork_head = %prepared.block_hash,
                        fork_depth = reorg_state.fork_length,
                        max_reorg_depth = reorg_state.depth,
                        "Switching forkchoice to reorg branch"
                    );
                }
            } else if let Some(reorg_state) = reorg_state.as_mut() &&
                next_target_branch == Some(ActiveBranch::Canonical)
            {
                reorg_state.active_branch = ActiveBranch::Canonical;

                if reorg_state.fork_length >= reorg_state.depth {
                    info!(
                        target: "reth-bench",
                        block_number,
                        reorg_depth = reorg_state.depth,
                        "Resetting reorg branch after switching back to canonical"
                    );
                    reorg_state.reset();
                }
            }

            let start = Instant::now();
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

            if forkchoice_state.head_block_hash != head {
                info!(
                    target: "reth-bench",
                    block_number,
                    canonical_head = %head,
                    active_head = %forkchoice_state.head_block_hash,
                    "Submitting forkchoice update to alternate branch"
                );
            }

            let fcu_start = Instant::now();
            call_forkchoice_updated_with_reth(&auth_provider, forkchoice_version, forkchoice_state)
                .await?;
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

            // Exclude time spent waiting on the block prefetch channel from the benchmark duration.
            // We want to measure engine throughput, not RPC fetch latency.
            blocks_processed += 1;
            let current_duration = total_benchmark_duration.elapsed() - total_wait_time;
            let progress = match total_blocks {
                Some(total) => format!("{blocks_processed}/{total}"),
                None => format!("{blocks_processed}"),
            };
            info!(target: "reth-bench", progress, %combined_result);

            if let Some(scraper) = metrics_scraper.as_mut() &&
                let Err(err) = scraper.scrape_after_block(block_number).await
            {
                warn!(target: "reth-bench", %err, block_number, "Failed to scrape metrics");
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
        }

        let (gas_output_results, combined_results): (Vec<TotalGasRow>, Vec<CombinedResult>) =
            results.into_iter().unzip();

        if let Some(ref path) = self.benchmark.output {
            write_benchmark_results(path, &gas_output_results, &combined_results)?;
        }

        if let (Some(path), Some(scraper)) = (&self.benchmark.output, &metrics_scraper) {
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
}

async fn prepare_built_block(
    block_provider: &RootProvider<AnyNetwork>,
    block: &AnyRpcBlock,
    parent_block_hash: B256,
    use_reth_namespace: bool,
    wait_for_persistence: WaitForPersistence,
    no_wait_for_caches: bool,
) -> eyre::Result<PreparedBuiltBlock> {
    let version = build_block_target_version(block)?;
    let request = build_block_request(block, parent_block_hash)?;
    let built_payload: ExecutionPayloadEnvelopeV5 =
        block_provider.client().request("testing_buildBlockV1", [request]).await.wrap_err_with(
            || format!("Failed to build block {} via testing_buildBlockV1", block.header.number),
        )?;

    let payload = &built_payload.execution_payload.payload_inner.payload_inner;
    let block_hash = payload.block_hash;
    let block_number = payload.block_number;
    let wait_for_persistence = wait_for_persistence.rpc_value(block_number);
    let (version, params) = built_payload_to_new_payload(
        built_payload,
        version,
        use_reth_namespace,
        wait_for_persistence,
        no_wait_for_caches,
        block.header.parent_beacon_block_root,
    )?;

    Ok(PreparedBuiltBlock { block_hash, version, params })
}

fn build_block_request(
    block: &AnyRpcBlock,
    parent_block_hash: B256,
) -> eyre::Result<TestingBuildBlockRequestV1> {
    let mut transactions = block
        .clone()
        .try_into_transactions()
        .map_err(|_| eyre::eyre!("Block transactions must be fetched in full for --reorg"))?
        .into_iter()
        .map(|tx| {
            let tx: TxEnvelope =
                tx.try_into().map_err(|_| eyre::eyre!("unsupported tx type in RPC block"))?;
            Ok::<Bytes, eyre::Report>(tx.encoded_2718().into())
        })
        .collect::<eyre::Result<Vec<_>>>()?;

    // Keep only 90% of the original transactions on the fork so the alternate branch produces a
    // materially different post-state instead of only differing by header data.
    let keep = transactions.len().saturating_mul(9) / 10;
    transactions.truncate(keep);

    let rpc_block = block.clone().into_inner();

    Ok(TestingBuildBlockRequestV1 {
        parent_block_hash,
        payload_attributes: PayloadAttributes {
            timestamp: block.header.timestamp,
            prev_randao: block.header.mix_hash.unwrap_or_default(),
            suggested_fee_recipient: block.header.beneficiary,
            withdrawals: rpc_block.withdrawals.map(|withdrawals| withdrawals.into_inner()),
            parent_beacon_block_root: block.header.parent_beacon_block_root,
            slot_number: block.header.slot_number,
        },
        transactions,
        extra_data: Some(block.header.extra_data.clone()),
    })
}

fn build_block_target_version(block: &AnyRpcBlock) -> eyre::Result<EngineApiMessageVersion> {
    if block.header.block_access_list_hash.is_some() || block.header.slot_number.is_some() {
        bail!("--reorg does not support Amsterdam payload fields with block access lists yet")
    }

    Ok(if block.header.requests_hash.is_some() {
        EngineApiMessageVersion::V4
    } else if block.header.parent_beacon_block_root.is_some() {
        EngineApiMessageVersion::V3
    } else if block.header.withdrawals_root.is_some() {
        EngineApiMessageVersion::V2
    } else {
        EngineApiMessageVersion::V1
    })
}

fn built_payload_to_new_payload(
    built_payload: ExecutionPayloadEnvelopeV5,
    target_version: EngineApiMessageVersion,
    use_reth_namespace: bool,
    wait_for_persistence: Option<bool>,
    no_wait_for_caches: bool,
    parent_beacon_block_root: Option<B256>,
) -> eyre::Result<(Option<EngineApiMessageVersion>, serde_json::Value)> {
    let execution_payload = built_payload.execution_payload.clone();

    match target_version {
        EngineApiMessageVersion::V1 => {
            let payload = execution_payload.payload_inner.payload_inner;
            if use_reth_namespace {
                let execution_data = ExecutionData {
                    payload: ExecutionPayload::V1(payload),
                    sidecar: ExecutionPayloadSidecar::none(),
                };
                Ok((
                    None,
                    reth_new_payload_params(
                        execution_data,
                        wait_for_persistence,
                        no_wait_for_caches,
                    )?,
                ))
            } else {
                Ok((Some(EngineApiMessageVersion::V1), serde_json::to_value((payload,))?))
            }
        }
        EngineApiMessageVersion::V2 => {
            let payload = execution_payload.payload_inner;
            if use_reth_namespace {
                let execution_data = ExecutionData {
                    payload: ExecutionPayload::V2(payload),
                    sidecar: ExecutionPayloadSidecar::none(),
                };
                Ok((
                    None,
                    reth_new_payload_params(
                        execution_data,
                        wait_for_persistence,
                        no_wait_for_caches,
                    )?,
                ))
            } else {
                let payload = ExecutionPayloadInputV2 {
                    execution_payload: payload.payload_inner,
                    withdrawals: Some(payload.withdrawals),
                };
                Ok((Some(EngineApiMessageVersion::V2), serde_json::to_value((payload,))?))
            }
        }
        EngineApiMessageVersion::V3 | EngineApiMessageVersion::V4 | EngineApiMessageVersion::V5 => {
            let parent_beacon_block_root = parent_beacon_block_root
                .ok_or_eyre("missing parent beacon block root for built fork block")?;
            let (payload, sidecar) =
                built_payload.into_payload_and_sidecar(parent_beacon_block_root);
            let (version, params, execution_data) =
                payload_to_new_payload(payload, sidecar, Some(target_version))?;

            if use_reth_namespace {
                Ok((
                    None,
                    reth_new_payload_params(
                        execution_data,
                        wait_for_persistence,
                        no_wait_for_caches,
                    )?,
                ))
            } else {
                Ok((Some(version), params))
            }
        }
        EngineApiMessageVersion::V6 => {
            bail!("--reorg does not support Amsterdam payload fields yet")
        }
    }
}

fn parse_reorg_depth(value: &str) -> Result<usize, String> {
    let depth = value
        .trim()
        .parse::<usize>()
        .map_err(|_| format!("invalid reorg depth {value:?}, expected a positive integer"))?;

    if depth == 0 {
        return Err("reorg depth must be greater than 0".to_string())
    }

    Ok(depth)
}

fn reth_new_payload_params(
    execution_data: ExecutionData,
    wait_for_persistence: Option<bool>,
    no_wait_for_caches: bool,
) -> eyre::Result<serde_json::Value> {
    Ok(serde_json::to_value((
        RethNewPayloadInput::ExecutionData(execution_data),
        wait_for_persistence,
        no_wait_for_caches.then_some(false),
    ))?)
}

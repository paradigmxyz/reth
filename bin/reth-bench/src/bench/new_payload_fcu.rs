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
    },
};
use alloy_consensus::TxEnvelope;
use alloy_eips::Encodable2718;
use alloy_primitives::B256;
use alloy_provider::{
    ext::DebugApi,
    network::{AnyNetwork, AnyRpcBlock},
    Provider, RootProvider,
};
use alloy_rpc_types_engine::{
    ExecutionData, ExecutionPayloadEnvelopeV5, ForkchoiceState, PayloadAttributes,
};
use clap::Parser;
use eyre::{bail, ensure, Context, OptionExt};
use futures::{stream, StreamExt, TryStreamExt};
use reth_cli_runner::CliContext;
use reth_engine_primitives::config::DEFAULT_PERSISTENCE_THRESHOLD;
use reth_node_core::args::BenchmarkArgs;
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
    params: serde_json::Value,
}

#[derive(Debug)]
struct QueuedForkBlock {
    block_number: u64,
    prepared: PreparedBuiltBlock,
}

#[derive(Debug)]
struct ReorgState {
    depth: usize,
    fork_length: usize,
    branch_point_hash: Option<B256>,
    fork_parent_hash: Option<B256>,
}

impl ReorgState {
    const fn new(depth: usize) -> Self {
        Self { depth, fork_length: 0, branch_point_hash: None, fork_parent_hash: None }
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
        let mut queued_fork_block = None;
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
            let deferred_branch_start_block = reorg_state
                .as_ref()
                .filter(|state| state.fork_length == 0 && queued_fork_block.is_none())
                .map(|_| block.clone());
            let canonical_forkchoice_state = ForkchoiceState {
                head_block_hash: head,
                safe_block_hash: safe,
                finalized_block_hash: finalized,
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

            let fcu_start = Instant::now();
            call_forkchoice_updated_with_reth(&auth_provider, version, canonical_forkchoice_state)
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

            if let Some(reorg_state) = reorg_state.as_mut() {
                if queued_fork_block.is_none() && reorg_state.fork_length == 0 {
                    // A branch start uses a canonical parent, so it can be built lazily here
                    // instead of being queued ahead of time.
                    let block = deferred_branch_start_block
                        .as_ref()
                        .ok_or_eyre("missing deferred fork block for reorg branch start")?;
                    queued_fork_block = Some(QueuedForkBlock {
                        block_number,
                        prepared: prepare_built_block(
                            &local_rpc_provider,
                            block,
                            canonical_parent_hash,
                            no_wait_for_caches,
                        )
                        .await?,
                    });
                }

                let queued = queued_fork_block
                    .take()
                    .ok_or_eyre("missing queued fork block for reorg replay")?;
                ensure!(
                    queued.block_number == block_number,
                    "queued fork block {} does not match source block {}",
                    queued.block_number,
                    block_number
                );
                let prepared = queued.prepared;

                call_new_payload_with_reth(&auth_provider, None, prepared.params).await?;

                reorg_state.push_fork_head(canonical_parent_hash, prepared.block_hash);
                let forkchoice_state = reorg_state.forkchoice_state(prepared.block_hash)?;

                info!(
                    target: "reth-bench",
                    block_number,
                    branch_point = %forkchoice_state.safe_block_hash,
                    fork_head = %prepared.block_hash,
                    fork_depth = reorg_state.fork_length,
                    max_reorg_depth = reorg_state.depth,
                    "Switching forkchoice to reorg branch"
                );

                let fcu_start = Instant::now();
                call_forkchoice_updated_with_reth(&auth_provider, None, forkchoice_state).await?;
                let _fork_fcu_latency = fcu_start.elapsed();

                let next_fork_block_number = block_number + 1;
                if reorg_state.fork_length < reorg_state.depth {
                    queued_fork_block = queue_fork_block(
                        &block_provider,
                        &local_rpc_provider,
                        &benchmark_mode,
                        next_fork_block_number,
                        Some(prepared.block_hash),
                        no_wait_for_caches,
                    )
                    .await?;
                } else {
                    info!(
                        target: "reth-bench",
                        block_number,
                        reorg_depth = reorg_state.depth,
                        "Resetting reorg branch after reaching max depth"
                    );

                    // `testing_buildBlockV1` resolves the parent from canonical state, so switch
                    // back to the source chain before reseeding the next queued fork block.
                    call_forkchoice_updated_with_reth(
                        &auth_provider,
                        version,
                        canonical_forkchoice_state,
                    )
                    .await?;

                    reorg_state.reset();
                    queued_fork_block = None;
                }
            }

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
    no_wait_for_caches: bool,
) -> eyre::Result<PreparedBuiltBlock> {
    const MAX_BUILD_ATTEMPTS: usize = 10;
    const BUILD_RETRY_INTERVAL: Duration = Duration::from_millis(100);

    let request = build_block_request(block, parent_block_hash)?;
    let built_payload: ExecutionPayloadEnvelopeV5 = {
        let mut attempts_remaining = MAX_BUILD_ATTEMPTS;

        loop {
            match block_provider.client().request("testing_buildBlockV1", [request.clone()]).await {
                Ok(payload) => break payload,
                Err(err) if attempts_remaining > 1 && is_retryable_build_block_error(&err) => {
                    warn!(
                        target: "reth-bench",
                        block_number = block.header.number,
                        %parent_block_hash,
                        attempts_remaining,
                        error = %err,
                        "Retrying testing_buildBlockV1 after transient fork build failure"
                    );
                    attempts_remaining -= 1;
                    tokio::time::sleep(BUILD_RETRY_INTERVAL).await;
                }
                Err(err) => {
                    return Err(err).wrap_err_with(|| {
                        format!(
                            "Failed to build block {} via testing_buildBlockV1",
                            block.header.number
                        )
                    })
                }
            }
        }
    };

    let payload = &built_payload.execution_payload.payload_inner.payload_inner;
    let block_hash = payload.block_hash;
    let (payload, sidecar) = built_payload
        .into_payload_and_sidecar(block.header.parent_beacon_block_root.unwrap_or_default());
    // Fork payloads are built immediately before the next `testing_buildBlockV1` call. Leaving
    // reth's default persistence wait enabled here gives the regular RPC side a consistent base
    // state for the next synthetic fork block build.
    let params = serde_json::to_value((
        RethNewPayloadInput::ExecutionData(ExecutionData { payload, sidecar }),
        None::<bool>,
        no_wait_for_caches.then_some(false),
    ))?;

    Ok(PreparedBuiltBlock { block_hash, params })
}

#[allow(clippy::too_many_arguments)]
async fn queue_fork_block(
    block_provider: &RootProvider<AnyNetwork>,
    local_rpc_provider: &RootProvider<AnyNetwork>,
    benchmark_mode: &crate::bench_mode::BenchMode,
    block_number: u64,
    parent_block_hash: Option<B256>,
    no_wait_for_caches: bool,
) -> eyre::Result<Option<QueuedForkBlock>> {
    if !benchmark_mode.contains(block_number) {
        return Ok(None)
    }

    let future_block = block_provider
        .get_block_by_number(alloy_eips::BlockNumberOrTag::Number(block_number))
        .full()
        .await
        .wrap_err_with(|| format!("Failed to fetch block by number {block_number}"))?
        .ok_or_eyre("Block not found")?;
    let parent_block_hash = parent_block_hash.unwrap_or(future_block.header.parent_hash);

    Ok(Some(QueuedForkBlock {
        block_number,
        prepared: prepare_built_block(
            local_rpc_provider,
            &future_block,
            parent_block_hash,
            no_wait_for_caches,
        )
        .await?,
    }))
}

fn is_retryable_build_block_error(err: &alloy_transport::TransportError) -> bool {
    let message = err.to_string();
    message.contains("block not found: hash") ||
        message.contains("block hash not found for block number")
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
            if tx.is_eip4844() {
                return Ok(None)
            }
            Ok(Some(tx.encoded_2718().into()))
        })
        .filter_map(|tx| tx.transpose())
        .collect::<eyre::Result<Vec<_>>>()?;

    // `testing_buildBlockV1` only takes raw transaction bytes, so we exclude blob transactions
    // from the synthetic fork blocks rather than trying to reconstruct their sidecars.
    // Keep only 90% of the remaining transactions so the alternate branch produces a materially
    // different post-state instead of only differing by header data.
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

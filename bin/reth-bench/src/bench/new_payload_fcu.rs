//! Runs the `reth bench` command, calling first newPayload for each block, then calling
//! forkchoiceUpdated.

use crate::{
    bench::{
        context::BenchContext,
        helpers::parse_duration,
        metrics_scraper::MetricsScraper,
        output::{
            write_benchmark_results, CombinedResult, NewPayloadResult, TotalGasOutput, TotalGasRow,
        },
    },
    valid_payload::{
        block_to_new_payload, call_forkchoice_updated_with_reth, call_new_payload_with_reth,
    },
};
use alloy_provider::{ext::DebugApi, Provider};
use alloy_rpc_types_engine::ForkchoiceState;
use clap::Parser;
use eyre::{Context, OptionExt};
use futures::{stream, StreamExt, TryStreamExt};
use reth_cli_runner::CliContext;
use reth_engine_primitives::config::DEFAULT_PERSISTENCE_THRESHOLD;
use reth_node_core::args::BenchmarkArgs;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

/// `reth benchmark new-payload-fcu` command
#[derive(Debug, Parser)]
pub struct Command {
    /// The RPC url to use for getting data.
    #[arg(long, value_name = "RPC_URL", verbatim_doc_comment)]
    rpc_url: String,

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

    #[command(flatten)]
    benchmark: BenchmarkArgs,
}

impl Command {
    /// Execute `benchmark new-payload-fcu` command
    pub async fn execute(self, _ctx: CliContext) -> eyre::Result<()> {
        // Log mode configuration
        if let Some(duration) = self.wait_time {
            info!(target: "reth-bench", "Using wait-time mode with {}ms minimum interval between blocks", duration.as_millis());
        }

        let BenchContext {
            benchmark_mode,
            block_provider,
            auth_provider,
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

        while let Some((block, head, safe, finalized, rlp)) = {
            let wait_start = Instant::now();
            let result = blocks.try_next().await?;
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

            let (version, params) = block_to_new_payload(
                block,
                rlp,
                use_reth_namespace,
                wait_for_persistence,
                no_wait_for_caches,
            )?;
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
            call_forkchoice_updated_with_reth(&auth_provider, version, forkchoice_state).await?;
            let fcu_latency = fcu_start.elapsed();

            let total_latency = if server_timings.is_some() {
                // When using server-side latency for newPayload, derive total from the
                // independently measured components to avoid mixing server-side and
                // client-side (network-inclusive) timings.
                np_latency + fcu_latency
            } else {
                start.elapsed()
            };
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

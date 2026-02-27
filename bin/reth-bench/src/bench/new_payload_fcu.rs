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
        helpers::parse_duration,
        metrics_scraper::MetricsScraper,
        output::{
            write_benchmark_results, CombinedResult, NewPayloadResult, TotalGasOutput, TotalGasRow,
        },
        persistence_waiter::{
            derive_ws_rpc_url, setup_persistence_subscription, PersistenceWaiter,
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

        // Set up waiter based on configured options
        // When both are set: wait at least wait_time, and also wait for persistence if needed
        let mut waiter = match (self.wait_time, self.wait_for_persistence) {
            (Some(duration), true) => {
                let ws_url = derive_ws_rpc_url(
                    self.benchmark.ws_rpc_url.as_deref(),
                    &self.benchmark.engine_rpc_url,
                )?;
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
                let ws_url = derive_ws_rpc_url(
                    self.benchmark.ws_rpc_url.as_deref(),
                    &self.benchmark.engine_rpc_url,
                )?;
                let sub = setup_persistence_subscription(ws_url, self.persistence_timeout).await?;
                Some(PersistenceWaiter::with_subscription(
                    sub,
                    self.persistence_threshold,
                    self.persistence_timeout,
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
            use_reth_namespace,
            rlp_blocks,
        } = BenchContext::new(&self.benchmark, self.rpc_url).await?;

        let total_blocks = benchmark_mode.total_blocks();

        let mut metrics_scraper = MetricsScraper::maybe_new(self.benchmark.metrics_url.clone());

        if use_reth_namespace {
            info!("Using reth_newPayload and reth_forkchoiceUpdated endpoints");
        }

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
                        tracing::error!(target: "reth-bench", "Failed to fetch block {next_block}: {e}");
                        let _ = error_sender.send(e);
                        break;
                    }
                };

                let rlp = if rlp_blocks {
                    let rlp = match block_provider.debug_get_raw_block(next_block.into()).await {
                        Ok(rlp) => rlp,
                        Err(e) => {
                            tracing::error!(target: "reth-bench", "Failed to fetch raw block {next_block}: {e}");
                            let _ = error_sender
                                .send(eyre::eyre!("Failed to fetch raw block {next_block}: {e}"));
                            break;
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
                    .send((block, head_block_hash, safe_block_hash, finalized_block_hash, rlp))
                    .await
                {
                    tracing::error!(target: "reth-bench", "Failed to send block data: {e}");
                    break;
                }
            }
        });

        let mut results = Vec::new();
        let mut blocks_processed = 0u64;
        let total_benchmark_duration = Instant::now();
        let mut total_wait_time = Duration::ZERO;

        while let Some((block, head, safe, finalized, rlp)) = {
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

            let (version, params) =
                block_to_new_payload(block, is_optimism, rlp, use_reth_namespace)?;
            let start = Instant::now();
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

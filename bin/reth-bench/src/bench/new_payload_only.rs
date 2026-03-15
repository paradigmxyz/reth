//! Runs the `reth bench` command, sending only newPayload, without a forkchoiceUpdated call.

use crate::{
    bench::{
        context::BenchContext,
        metrics_scraper::MetricsScraper,
        output::{
            NewPayloadResult, TotalGasOutput, TotalGasRow, GAS_OUTPUT_SUFFIX,
            NEW_PAYLOAD_OUTPUT_SUFFIX,
        },
    },
    valid_payload::{block_to_new_payload, call_new_payload_with_reth},
};
use alloy_primitives::Bytes;
use alloy_provider::{
    ext::DebugApi,
    network::{AnyNetwork, AnyRpcBlock},
    Provider, RootProvider,
};
use clap::Parser;
use csv::Writer;
use eyre::{Context, OptionExt};
use futures::stream::{FuturesOrdered, StreamExt};
use reth_cli_runner::CliContext;
use reth_node_core::args::BenchmarkArgs;
use std::time::{Duration, Instant};
use tracing::{debug, info};

/// `reth benchmark new-payload-only` command
#[derive(Debug, Parser)]
pub struct Command {
    /// The RPC url to use for getting data.
    #[arg(long, value_name = "RPC_URL", verbatim_doc_comment)]
    rpc_url: String,

    /// The size of the block buffer (channel capacity) for prefetching blocks from the RPC
    /// endpoint.
    #[arg(
        long = "rpc-block-buffer-size",
        value_name = "RPC_BLOCK_BUFFER_SIZE",
        default_value = "20",
        verbatim_doc_comment
    )]
    rpc_block_buffer_size: usize,

    /// Number of blocks to fetch concurrently from the RPC endpoint.
    ///
    /// When greater than 1, multiple blocks are fetched in parallel within the background
    /// prefetch task, keeping the block buffer full even when RPC latency is high. Results
    /// are always sent to the engine in order.
    #[arg(
        long = "rpc-parallel-fetches",
        value_name = "RPC_PARALLEL_FETCHES",
        default_value = "1",
        verbatim_doc_comment
    )]
    rpc_parallel_fetches: usize,

    #[command(flatten)]
    benchmark: BenchmarkArgs,
}

impl Command {
    /// Execute `benchmark new-payload-only` command
    pub async fn execute(self, _ctx: CliContext) -> eyre::Result<()> {
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
            info!("Using reth_newPayload endpoint");
        }

        let buffer_size = self.rpc_block_buffer_size;
        let parallel_fetches = self.rpc_parallel_fetches.max(1);

        if parallel_fetches > 1 {
            info!(
                target: "reth-bench",
                parallel_fetches,
                buffer_size,
                "Parallel block prefetching enabled"
            );
        }

        // Use a oneshot channel to propagate errors from the spawned task
        let (error_sender, mut error_receiver) = tokio::sync::oneshot::channel();
        let (sender, mut receiver) = tokio::sync::mpsc::channel(buffer_size);

        tokio::task::spawn(async move {
            let mut futures = FuturesOrdered::new();

            // Seed the pipeline with up to `parallel_fetches` concurrent requests
            for _ in 0..parallel_fetches {
                if benchmark_mode.contains(next_block) {
                    let block_num = next_block;
                    next_block += 1;
                    futures.push_back(fetch_block_only(&block_provider, block_num, rlp_blocks));
                }
            }

            while let Some(result) = futures.next().await {
                let data = match result {
                    Ok(data) => data,
                    Err(e) => {
                        tracing::error!(target: "reth-bench", %e, "Block fetch failed");
                        let _ = error_sender.send(e);
                        break;
                    }
                };

                // Queue the next fetch before sending to keep the pipeline full
                if benchmark_mode.contains(next_block) {
                    let block_num = next_block;
                    next_block += 1;
                    futures.push_back(fetch_block_only(&block_provider, block_num, rlp_blocks));
                }

                if sender.send(data).await.is_err() {
                    tracing::error!(target: "reth-bench", "Channel closed, stopping prefetch");
                    break;
                }
            }
        });

        let mut results = Vec::new();
        let mut blocks_processed = 0u64;
        let total_benchmark_duration = Instant::now();
        let mut total_wait_time = Duration::ZERO;

        while let Some((block, rlp)) = {
            let wait_start = Instant::now();
            let result = receiver.recv().await;
            total_wait_time += wait_start.elapsed();
            result
        } {
            let block_number = block.header.number;
            let transaction_count = block.transactions.len() as u64;
            let gas_used = block.header.gas_used;

            debug!(target: "reth-bench", number=?block.header.number, "Sending payload to engine");

            let (version, params) =
                block_to_new_payload(block, is_optimism, rlp, use_reth_namespace)?;

            let start = Instant::now();
            let server_timings =
                call_new_payload_with_reth(&auth_provider, version, params).await?;

            let latency =
                server_timings.as_ref().map(|t| t.latency).unwrap_or_else(|| start.elapsed());
            let new_payload_result = NewPayloadResult {
                gas_used,
                latency,
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
            blocks_processed += 1;
            let progress = match total_blocks {
                Some(total) => format!("{blocks_processed}/{total}"),
                None => format!("{blocks_processed}"),
            };
            info!(target: "reth-bench", progress, %new_payload_result);

            // current duration since the start of the benchmark minus the time
            // waiting for blocks
            let current_duration = total_benchmark_duration.elapsed() - total_wait_time;

            // record the current result
            let row =
                TotalGasRow { block_number, transaction_count, gas_used, time: current_duration };
            results.push((row, new_payload_result));

            if let Some(scraper) = metrics_scraper.as_mut() &&
                let Err(err) = scraper.scrape_after_block(block_number).await
            {
                tracing::warn!(target: "reth-bench", %err, block_number, "Failed to scrape metrics");
            }
        }

        // Check if the spawned task encountered an error
        if let Ok(error) = error_receiver.try_recv() {
            return Err(error);
        }

        let (gas_output_results, new_payload_results): (_, Vec<NewPayloadResult>) =
            results.into_iter().unzip();

        // write the csv output to files
        if let Some(path) = self.benchmark.output {
            // first write the new payload results to a file
            let output_path = path.join(NEW_PAYLOAD_OUTPUT_SUFFIX);
            info!(target: "reth-bench", "Writing newPayload call latency output to file: {:?}", output_path);
            let mut writer = Writer::from_path(output_path)?;
            for result in new_payload_results {
                writer.serialize(result)?;
            }
            writer.flush()?;

            // now write the gas output to a file
            let output_path = path.join(GAS_OUTPUT_SUFFIX);
            info!(target: "reth-bench", "Writing total gas output to file: {:?}", output_path);
            let mut writer = Writer::from_path(output_path)?;
            for row in &gas_output_results {
                writer.serialize(row)?;
            }
            writer.flush()?;

            if let Some(scraper) = &metrics_scraper {
                scraper.write_csv(&path)?;
            }

            info!(target: "reth-bench", "Finished writing benchmark output files to {:?}.", path);
        }

        // accumulate the results and calculate the overall Ggas/s
        let gas_output = TotalGasOutput::new(gas_output_results)?;
        info!(
            target: "reth-bench",
            total_duration=?gas_output.total_duration,
            total_gas_used=?gas_output.total_gas_used,
            blocks_processed=?gas_output.blocks_processed,
            "Total Ggas/s: {:.4}",
            gas_output.total_gigagas_per_second()
        );

        Ok(())
    }
}

/// Fetches a single block (and optionally its RLP) from the RPC provider.
async fn fetch_block_only(
    provider: &RootProvider<AnyNetwork>,
    block_number: u64,
    rlp_blocks: bool,
) -> eyre::Result<(AnyRpcBlock, Option<Bytes>)> {
    let block = provider
        .get_block_by_number(block_number.into())
        .full()
        .await
        .wrap_err_with(|| format!("Failed to fetch block by number {block_number}"))?
        .ok_or_eyre(format!("Block {block_number} not found"))?;

    let rlp = if rlp_blocks {
        Some(
            provider
                .debug_get_raw_block(block_number.into())
                .await
                .map_err(|e| eyre::eyre!("Failed to fetch raw block {block_number}: {e}"))?,
        )
    } else {
        None
    };

    Ok((block, rlp))
}

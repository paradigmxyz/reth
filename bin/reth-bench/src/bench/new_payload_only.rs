//! Runs the `reth bench` command, sending only newPayload, without a forkchoiceUpdated call.
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
            NewPayloadResult, TotalGasOutput, TotalGasRow, GAS_OUTPUT_SUFFIX,
            NEW_PAYLOAD_OUTPUT_SUFFIX,
        },
        persistence_waiter::{
            engine_url_to_ws_url, setup_persistence_subscription, PersistenceWaiter,
            PERSISTENCE_CHECKPOINT_TIMEOUT,
        },
    },
    valid_payload::{block_to_new_payload, call_new_payload},
};
use alloy_provider::Provider;
use clap::Parser;
use csv::Writer;
use eyre::{Context, OptionExt};
use humantime::parse_duration;
use reth_cli_runner::CliContext;
use reth_engine_primitives::config::DEFAULT_PERSISTENCE_THRESHOLD;
use reth_node_core::args::BenchmarkArgs;
use std::time::{Duration, Instant};
use tracing::{debug, info};
use url::Url;

/// `reth benchmark new-payload-only` command
#[derive(Debug, Parser)]
pub struct Command {
    /// The RPC url to use for getting data.
    #[arg(long, value_name = "RPC_URL", verbatim_doc_comment)]
    rpc_url: String,

    /// How long to wait after calling newPayload before sending the next payload.
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
    /// Execute `benchmark new-payload-only` command
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
                let ws_url = self.derive_ws_rpc_url()?;
                let sub = setup_persistence_subscription(ws_url).await?;
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

                next_block += 1;
                if let Err(e) = sender.send(block).await {
                    tracing::error!("Failed to send block data: {e}");
                    break;
                }
            }
        });

        // put results in a summary vec so they can be printed at the end
        let mut results = Vec::new();
        let total_benchmark_duration = Instant::now();
        let mut total_wait_time = Duration::ZERO;

        while let Some(block) = {
            let wait_start = Instant::now();
            let result = receiver.recv().await;
            total_wait_time += wait_start.elapsed();
            result
        } {
            let block_number = block.header.number;
            let transaction_count = block.transactions.len() as u64;
            let gas_used = block.header.gas_used;

            debug!(number=?block.header.number, "Sending payload to engine");

            let (version, params) = block_to_new_payload(block, is_optimism)?;

            let start = Instant::now();
            call_new_payload(&auth_provider, version, params).await?;

            let new_payload_result = NewPayloadResult { gas_used, latency: start.elapsed() };
            info!(%new_payload_result);

            if let Some(w) = &mut waiter {
                w.on_block(block_number).await?;
            }

            // current duration since the start of the benchmark minus the time
            // waiting for blocks
            let current_duration = total_benchmark_duration.elapsed() - total_wait_time;

            // record the current result
            let row =
                TotalGasRow { block_number, transaction_count, gas_used, time: current_duration };
            results.push((row, new_payload_result));
        }

        // Check if the spawned task encountered an error
        if let Ok(error) = error_receiver.try_recv() {
            return Err(error);
        }

        // Drop waiter - we don't need to wait for final blocks to persist
        // since the benchmark goal is measuring Ggas/s of newPayload, not persistence.
        drop(waiter);

        let (gas_output_results, new_payload_results): (_, Vec<NewPayloadResult>) =
            results.into_iter().unzip();

        // write the csv output to files
        if let Some(path) = self.benchmark.output {
            // first write the new payload results to a file
            let output_path = path.join(NEW_PAYLOAD_OUTPUT_SUFFIX);
            info!("Writing newPayload call latency output to file: {:?}", output_path);
            let mut writer = Writer::from_path(output_path)?;
            for result in new_payload_results {
                writer.serialize(result)?;
            }
            writer.flush()?;

            // now write the gas output to a file
            let output_path = path.join(GAS_OUTPUT_SUFFIX);
            info!("Writing total gas output to file: {:?}", output_path);
            let mut writer = Writer::from_path(output_path)?;
            for row in &gas_output_results {
                writer.serialize(row)?;
            }
            writer.flush()?;

            info!("Finished writing benchmark output files to {:?}.", path);
        }

        // accumulate the results and calculate the overall Ggas/s
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
}

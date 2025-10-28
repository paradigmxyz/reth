//! Runs the `reth bench` command, sending only newPayload, without a forkchoiceUpdated call.

use crate::{
    bench::{
        block_storage::BlockFileReader,
        context::{BenchContext, BlockSource},
        output::{
            NewPayloadResult, TotalGasOutput, TotalGasRow, GAS_OUTPUT_SUFFIX,
            NEW_PAYLOAD_OUTPUT_SUFFIX,
        },
    },
    valid_payload::{block_to_new_payload, call_new_payload, consensus_block_to_new_payload},
};
use alloy_consensus::Header;
use alloy_provider::Provider;
use clap::Parser;
use csv::Writer;
use eyre::{Context, OptionExt};
use reth_cli_runner::CliContext;
use reth_node_core::args::BenchmarkArgs;
use std::time::{Duration, Instant};
use tracing::{debug, info};

/// `reth benchmark new-payload-only` command
#[derive(Debug, Parser)]
pub struct Command {
    /// The RPC url to use for getting data.
    #[arg(long, value_name = "RPC_URL", verbatim_doc_comment)]
    rpc_url: Option<String>,

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
        let BenchContext { block_source, auth_provider, is_optimism } =
            BenchContext::new(&self.benchmark, self.rpc_url).await?;

        let buffer_size = self.rpc_block_buffer_size;

        let auth_provider =
            auth_provider.ok_or_eyre("--jwt-secret must be provided for authenticated RPC")?;

        // Use a oneshot channel to propagate errors from the spawned task
        let (error_sender, mut error_receiver) = tokio::sync::oneshot::channel();
        let (sender, mut receiver) = tokio::sync::mpsc::channel(buffer_size);

        // Spawn task based on block source
        match block_source {
            BlockSource::File { path } => {
                // Use file reader for blocks
                tokio::task::spawn(async move {
                    let mut reader = match BlockFileReader::new(&path, buffer_size) {
                        Ok(r) => r,
                        Err(e) => {
                            tracing::error!("Failed to create block reader: {e}");
                            let _ = error_sender.send(e);
                            return;
                        }
                    };

                    // At anypoint if we fail to process a block, or read a batch of blocks, it
                    // means the file is corrupted somehow so the benchmark should exit as the
                    // results are invalidated
                    loop {
                        let blocks = match reader.read_batch() {
                            Ok(batch) if batch.is_empty() => break, // EOF
                            Ok(batch) => batch,
                            Err(e) => {
                                tracing::error!("Failed to read blocks from file: {e}");
                                let _ = error_sender.send(e);
                                return;
                            }
                        };
                        for block in blocks {
                            let header: Header = block.header.clone();
                            let (version, params) =
                                match consensus_block_to_new_payload(block, is_optimism) {
                                    Ok(result) => result,
                                    Err(e) => {
                                        tracing::error!(
                                            "Failed to convert block to new payload: {e}"
                                        );
                                        let _ = error_sender.send(e);
                                        return;
                                    }
                                };

                            if let Err(e) = sender.send((header, version, params)).await {
                                tracing::error!("Failed to send block data: {e}");
                                return;
                            }
                        }
                    }
                });
            }
            BlockSource::Rpc { provider, mut next_block, mode } => {
                // Use RPC provider for blocks
                tokio::task::spawn(async move {
                    while mode.contains(next_block) {
                        let block_res = provider
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
                                    tracing::error!("Failed to fetch block {next_block}: {e}");
                                    let _ = error_sender.send(e);
                                    break;
                                }
                            };
                        let header = block.header.clone().inner.into_header_with_defaults();

                        let (version, params) = match block_to_new_payload(block, is_optimism) {
                            Ok(result) => result,
                            Err(e) => {
                                tracing::error!("Failed to convert block to new payload: {e}");
                                let _ = error_sender.send(e);
                                break;
                            }
                        };

                        next_block += 1;
                        if let Err(e) = sender.send((header, version, params)).await {
                            tracing::error!("Failed to send block data: {e}");
                            break;
                        }
                    }
                });
            }
        }

        // put results in a summary vec so they can be printed at the end
        let mut results = Vec::new();
        let total_benchmark_duration = Instant::now();
        let mut total_wait_time = Duration::ZERO;

        while let Some((header, version, params)) = {
            let wait_start = Instant::now();
            let result = receiver.recv().await;
            total_wait_time += wait_start.elapsed();
            result
        } {
            // just put gas used here
            let gas_used = header.gas_used;

            let block_number = header.number;

            debug!(
                target: "reth-bench",
                number=?header.number,
                "Sending payload to engine",
            );

            let start = Instant::now();
            call_new_payload(&auth_provider, version, params).await?;

            let new_payload_result = NewPayloadResult { gas_used, latency: start.elapsed() };
            info!(%new_payload_result);

            // current duration since the start of the benchmark minus the time
            // waiting for blocks
            let current_duration = total_benchmark_duration.elapsed() - total_wait_time;

            // record the current result
            let row = TotalGasRow { block_number, gas_used, time: current_duration };
            results.push((row, new_payload_result));
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
}

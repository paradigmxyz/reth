//! Runs the `reth bench` command, calling first newPayload for each block, then calling
//! forkchoiceUpdated.

use crate::{
    bench::{
        block_storage::BlockFileReader,
        context::BenchContext,
        output::{
            CombinedResult, NewPayloadResult, TotalGasOutput, TotalGasRow, COMBINED_OUTPUT_SUFFIX,
            GAS_OUTPUT_SUFFIX,
        },
    },
    valid_payload::{
        block_to_new_payload, call_forkchoice_updated, call_new_payload,
        consensus_block_to_new_payload,
    },
};
use alloy_primitives::BlockHash;
use alloy_provider::Provider;
use alloy_rpc_types_engine::ForkchoiceState;
use clap::Parser;
use csv::Writer;
use eyre::{Context, OptionExt, Report};
use humantime::parse_duration;
use reth_cli_runner::CliContext;
use reth_node_api::EngineApiMessageVersion;
use reth_node_core::args::BenchmarkArgs;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use tracing::{debug, info};

/// Message sent through the channel containing block data ready for engine API submission
struct BlockPayload {
    /// Block number for logging
    block_number: u64,
    /// Gas used in the block
    gas_used: u64,
    /// Engine API message version
    version: EngineApiMessageVersion,
    /// Serialized payload parameters
    params: serde_json::Value,
    /// Head block hash for forkchoice
    head_block_hash: BlockHash,
    /// Safe block hash for forkchoice
    safe_block_hash: BlockHash,
    /// Finalized block hash for forkchoice
    finalized_block_hash: BlockHash,
}

/// `reth benchmark new-payload-fcu` command
#[derive(Debug, Parser)]
pub struct Command {
    /// The RPC url to use for getting data.
    #[arg(long, value_name = "RPC_URL", verbatim_doc_comment)]
    rpc_url: Option<String>,

    /// How long to wait after a forkchoice update before sending the next payload.
    #[arg(long, value_name = "WAIT_TIME", value_parser = parse_duration, verbatim_doc_comment)]
    wait_time: Option<Duration>,

    /// The size of the block buffer (channel capacity) for prefetching blocks from the RPC
    /// endpoint.
    #[arg(
        long = "rpc-block-buffer-size",
        value_name = "RPC_BLOCK_BUFFER_SIZE",
        default_value = "20",
        verbatim_doc_comment
    )]
    rpc_block_buffer_size: usize,

    /// The size of the file block buffer for writing the blocks from the file
    #[arg(
        long = "file-block-buffer-size",
        value_name = "FILE_BLOCK_BUFFER_SIZE",
        default_value = "100",
        verbatim_doc_comment
    )]
    file_block_buffer_size: usize,

    #[command(flatten)]
    benchmark: BenchmarkArgs,
}

impl Command {
    /// Execute `benchmark new-payload-fcu` command
    pub async fn execute(self, _ctx: CliContext) -> eyre::Result<()> {
        // Determine if we're using file or RPC source
        if let Some(file_path) = &self.benchmark.from_file {
            info!("Using file-based block source: {:?}", file_path);
            let fp = file_path.clone();
            self.execute_from_file(fp).await
        } else {
            let rpc_url = self
                .rpc_url
                .as_ref()
                .ok_or_else(|| eyre::eyre!("--rpc-url is required when not using --from-file"))?;
            info!("Using RPC-based block source: {}", rpc_url);
            self.execute_from_rpc().await
        }
    }

    /// Execute benchmark using blocks from a file
    async fn execute_from_file(self, file_path: PathBuf) -> eyre::Result<()> {
        // Get context - this will auto-detect if it's Optimism from the file
        let BenchContext { auth_provider, is_optimism, .. } =
            BenchContext::new(&self.benchmark, None).await?;

        let buffer_size = self.file_block_buffer_size;

        let mut block_reader = BlockFileReader::new(&file_path, buffer_size, is_optimism)?;

        let (error_sender, error_receiver) = tokio::sync::oneshot::channel();
        let (sender, receiver) = tokio::sync::mpsc::channel(buffer_size);

        // Spawn task to read blocks from file
        tokio::task::spawn(async move {
            loop {
                // Read batch of blocks
                let batch = match block_reader.read_batch() {
                    Ok(batch) => batch,
                    Err(e) => {
                        tracing::error!("Failed to read block batch: {e}");
                        let _ = error_sender.send(e);
                        break;
                    }
                };

                if batch.is_empty() {
                    info!("Finished reading all blocks from file");
                    break;
                }

                // Process each block in the batch
                for block in batch {
                    let header = block.header().clone();
                    let head_block_hash = header.hash_slow();
                    let gas_used = header.gas_used;
                    let block_number = header.number;

                    let (version, params) = match consensus_block_to_new_payload(
                        block.into_either_block(),
                        is_optimism,
                    ) {
                        Ok(result) => result,
                        Err(e) => {
                            tracing::error!("Failed to convert block to new payload: {e}");
                            break;
                        }
                    };

                    // For file-based, we'll use the same hash for safe and finalized
                    let payload = BlockPayload {
                        block_number,
                        gas_used,
                        version,
                        params,
                        head_block_hash,
                        safe_block_hash: head_block_hash,
                        finalized_block_hash: head_block_hash,
                    };

                    if let Err(e) = sender.send(payload).await {
                        tracing::error!("Failed to send block data: {e}");
                        return;
                    }
                }
            }
        });

        self.process_blocks(receiver, error_receiver, auth_provider).await
    }

    /// Execute benchmark using blocks from RPC
    async fn execute_from_rpc(self) -> eyre::Result<()> {
        let rpc_url = self
            .rpc_url
            .clone()
            .ok_or_else(|| eyre::eyre!("--rpc-url is required when not using --from-file"))?;

        let BenchContext {
            benchmark_mode,
            block_provider,
            auth_provider,
            mut next_block,
            is_optimism,
        } = BenchContext::new(&self.benchmark, Some(rpc_url)).await?;

        let buffer_size = self.rpc_block_buffer_size;

        // Use a oneshot channel to propagate errors from the spawned task
        let (error_sender, error_receiver) = tokio::sync::oneshot::channel();
        let (sender, receiver) = tokio::sync::mpsc::channel(buffer_size);

        let block_provider = block_provider
            .ok_or_else(|| eyre::eyre!("Block provider should be available for RPC mode"))?;

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
                let header = block.header.clone();
                let gas_used = header.gas_used;
                let block_number = header.number;

                let (version, params) = match block_to_new_payload(block, is_optimism) {
                    Ok(result) => result,
                    Err(e) => {
                        tracing::error!("Failed to convert block to new payload: {e}");
                        let _ = error_sender.send(e);
                        break;
                    }
                };
                let head_block_hash = header.hash;
                let safe_block_hash =
                    block_provider.get_block_by_number(header.number.saturating_sub(32).into());

                let finalized_block_hash =
                    block_provider.get_block_by_number(header.number.saturating_sub(64).into());

                let (safe, finalized) = tokio::join!(safe_block_hash, finalized_block_hash);

                let safe_block_hash = match safe {
                    Ok(Some(block)) => block.header.hash,
                    Ok(None) | Err(_) => head_block_hash,
                };

                let finalized_block_hash = match finalized {
                    Ok(Some(block)) => block.header.hash,
                    Ok(None) | Err(_) => head_block_hash,
                };

                let payload = BlockPayload {
                    block_number,
                    gas_used,
                    version,
                    params,
                    head_block_hash,
                    safe_block_hash,
                    finalized_block_hash,
                };

                next_block += 1;
                if let Err(e) = sender.send(payload).await {
                    tracing::error!("Failed to send block data: {e}");
                    break;
                }
            }
        });

        self.process_blocks(receiver, error_receiver, auth_provider).await
    }

    /// Common block processing logic for both RPC and file sources
    async fn process_blocks(
        self,
        mut receiver: tokio::sync::mpsc::Receiver<BlockPayload>,
        mut error_receiver: tokio::sync::oneshot::Receiver<Report>,
        auth_provider: alloy_provider::RootProvider<alloy_provider::network::AnyNetwork>,
    ) -> eyre::Result<()> {
        // put results in a summary vec so they can be printed at the end
        let mut results = Vec::new();
        let total_benchmark_duration = Instant::now();
        let mut total_wait_time = Duration::ZERO;

        while let Some(payload) = {
            let wait_start = Instant::now();
            let result = receiver.recv().await;
            total_wait_time += wait_start.elapsed();
            result
        } {
            let BlockPayload {
                block_number,
                gas_used,
                version,
                params,
                head_block_hash,
                safe_block_hash,
                finalized_block_hash,
            } = payload;

            debug!(target: "reth-bench", ?block_number, "Sending payload");

            // construct fcu to call
            let forkchoice_state =
                ForkchoiceState { head_block_hash, safe_block_hash, finalized_block_hash };

            let start = Instant::now();
            call_new_payload(&auth_provider, version, params).await?;

            let new_payload_result = NewPayloadResult { gas_used, latency: start.elapsed() };

            call_forkchoice_updated(&auth_provider, version, forkchoice_state, None).await?;

            // calculate the total duration and the fcu latency, record
            let total_latency = start.elapsed();
            let fcu_latency = total_latency - new_payload_result.latency;
            let combined_result =
                CombinedResult { block_number, new_payload_result, fcu_latency, total_latency };

            // current duration since the start of the benchmark minus the time
            // waiting for blocks
            let current_duration = total_benchmark_duration.elapsed() - total_wait_time;

            // convert gas used to gigagas, then compute gigagas per second
            info!(%combined_result);

            // wait if we need to
            if let Some(wait_time) = self.wait_time {
                tokio::time::sleep(wait_time).await;
            }

            // record the current result
            let gas_row = TotalGasRow { block_number, gas_used, time: current_duration };
            results.push((gas_row, combined_result));
        }

        // Check if the spawned task encountered an error
        if let Ok(error) = error_receiver.try_recv() {
            return Err(error);
        }

        let (gas_output_results, combined_results): (_, Vec<CombinedResult>) =
            results.into_iter().unzip();

        // write the csv output to files
        if let Some(path) = self.benchmark.output {
            // first write the combined results to a file
            let output_path = path.join(COMBINED_OUTPUT_SUFFIX);
            info!("Writing engine api call latency output to file: {:?}", output_path);
            let mut writer = Writer::from_path(output_path)?;
            for result in combined_results {
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

//! Runs the `reth bench` command, calling first newPayload for each block, then calling
//! forkchoiceUpdated.

use crate::{
    bench::{
        context::BenchContext,
        output::{
            CombinedResult, NewPayloadResult, TotalGasOutput, TotalGasRow, COMBINED_OUTPUT_SUFFIX,
            GAS_OUTPUT_SUFFIX,
        },
    },
    valid_payload::{block_to_new_payload, call_forkchoice_updated, call_new_payload},
};
use alloy_provider::Provider;
use alloy_rpc_types_engine::ForkchoiceState;
use clap::Parser;
use csv::Writer;
use eyre::{Context, OptionExt};
use humantime::parse_duration;
use reth_cli_runner::CliContext;
use reth_node_core::args::BenchmarkArgs;
use std::time::{Duration, Instant};
use tracing::{debug, info};

/// `reth benchmark new-payload-fcu` command
#[derive(Debug, Parser)]
pub struct Command {
    /// The RPC url to use for getting data.
    #[arg(long, value_name = "RPC_URL", verbatim_doc_comment)]
    rpc_url: String,

    /// How long to wait after a forkchoice update before sending the next payload.
    #[arg(long, value_name = "WAIT_TIME", value_parser = parse_duration, default_value = "250ms", verbatim_doc_comment)]
    wait_time: Duration,

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
        let BenchContext {
            benchmark_mode,
            block_provider,
            auth_provider,
            mut next_block,
            is_optimism,
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
                let header = block.header.clone();

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
                    .send((
                        header,
                        version,
                        params,
                        head_block_hash,
                        safe_block_hash,
                        finalized_block_hash,
                    ))
                    .await
                {
                    tracing::error!("Failed to send block data: {e}");
                    break;
                }
            }
        });

        // put results in a summary vec so they can be printed at the end
        let mut results = Vec::new();
        let total_benchmark_duration = Instant::now();
        let mut total_wait_time = Duration::ZERO;

        while let Some((header, version, params, head, safe, finalized)) = {
            let wait_start = Instant::now();
            let result = receiver.recv().await;
            total_wait_time += wait_start.elapsed();
            result
        } {
            // just put gas used here
            let gas_used = header.gas_used;
            let block_number = header.number;

            debug!(target: "reth-bench", ?block_number, "Sending payload",);

            // construct fcu to call
            let forkchoice_state = ForkchoiceState {
                head_block_hash: head,
                safe_block_hash: safe,
                finalized_block_hash: finalized,
            };

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

            // wait before sending the next payload
            tokio::time::sleep(self.wait_time).await;

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

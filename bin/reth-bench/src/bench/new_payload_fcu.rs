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
    valid_payload::{call_forkchoice_updated, call_new_payload},
};
use alloy_primitives::B256;
use alloy_provider::Provider;
use alloy_rpc_types_engine::ForkchoiceState;
use clap::Parser;
use csv::Writer;
use reth_cli_runner::CliContext;
use reth_node_core::args::BenchmarkArgs;
use reth_primitives::Block;
use reth_rpc_types_compat::engine::payload::block_to_payload;
use std::time::Instant;
use tracing::{debug, info};

/// `reth benchmark new-payload-fcu` command
#[derive(Debug, Parser)]
pub struct Command {
    /// The RPC url to use for getting data.
    #[arg(long, value_name = "RPC_URL", verbatim_doc_comment)]
    rpc_url: String,

    #[command(flatten)]
    benchmark: BenchmarkArgs,
}

impl Command {
    /// Execute `benchmark new-payload-fcu` command
    pub async fn execute(self, _ctx: CliContext) -> eyre::Result<()> {
        let cloned_args = self.benchmark.clone();
        let BenchContext { benchmark_mode, block_provider, auth_provider, mut next_block } =
            BenchContext::new(&cloned_args, self.rpc_url).await?;

        let (sender, mut receiver) = tokio::sync::mpsc::channel(1000);
        tokio::task::spawn(async move {
            while benchmark_mode.contains(next_block) {
                let block_res = block_provider.get_block_by_number(next_block.into(), true).await;
                let block = block_res.unwrap().unwrap();
                let block_hash = block.header.hash;
                let block = Block::try_from(block.inner).unwrap().seal(block_hash);
                let head_block_hash = block.hash();
                let safe_block_hash = block_provider
                    .get_block_by_number(block.number.saturating_sub(32).into(), false);

                let finalized_block_hash = block_provider
                    .get_block_by_number(block.number.saturating_sub(64).into(), false);

                let (safe, finalized) = tokio::join!(safe_block_hash, finalized_block_hash,);

                let safe_block_hash = safe.unwrap().expect("finalized block exists").header.hash;
                let finalized_block_hash =
                    finalized.unwrap().expect("finalized block exists").header.hash;

                next_block += 1;
                sender
                    .send((block, head_block_hash, safe_block_hash, finalized_block_hash))
                    .await
                    .unwrap();
            }
        });

        // put results in a summary vec so they can be printed at the end
        let mut results = Vec::new();
        let total_benchmark_duration = Instant::now();

        while let Some((block, head, safe, finalized)) = receiver.recv().await {
            // just put gas used here
            let gas_used = block.header.gas_used;
            let block_number = block.header.number;

            let versioned_hashes: Vec<B256> =
                block.blob_versioned_hashes().into_iter().copied().collect();
            let parent_beacon_block_root = block.parent_beacon_block_root;
            let payload = block_to_payload(block);

            debug!(?block_number, "Sending payload",);

            // construct fcu to call
            let forkchoice_state = ForkchoiceState {
                head_block_hash: head,
                safe_block_hash: safe,
                finalized_block_hash: finalized,
            };

            let start = Instant::now();
            let message_version = call_new_payload(
                &auth_provider,
                payload,
                parent_beacon_block_root,
                versioned_hashes,
            )
            .await?;

            let new_payload_result = NewPayloadResult { gas_used, latency: start.elapsed() };

            call_forkchoice_updated(&auth_provider, message_version, forkchoice_state, None)
                .await?;

            // calculate the total duration and the fcu latency, record
            let total_latency = start.elapsed();
            let fcu_latency = total_latency - new_payload_result.latency;
            let combined_result =
                CombinedResult { block_number, new_payload_result, fcu_latency, total_latency };

            // current duration since the start of the benchmark
            let current_duration = total_benchmark_duration.elapsed();

            // convert gas used to gigagas, then compute gigagas per second
            info!(%combined_result);

            // record the current result
            let gas_row = TotalGasRow { block_number, gas_used, time: current_duration };
            results.push((gas_row, combined_result));
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
        let gas_output = TotalGasOutput::new(gas_output_results);
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

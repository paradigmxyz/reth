//! Runs the `reth bench` command, sending only newPayload, without a forkchoiceUpdated call.

use crate::{
    bench::{
        context::BenchContext,
        output::{
            NewPayloadResult, TotalGasOutput, TotalGasRow, GAS_OUTPUT_SUFFIX,
            NEW_PAYLOAD_OUTPUT_SUFFIX,
        },
    },
    valid_payload::call_new_payload,
};
use alloy_primitives::B256;
use alloy_provider::Provider;
use clap::Parser;
use csv::Writer;
use reth_cli_runner::CliContext;
use reth_node_core::args::BenchmarkArgs;
use reth_primitives::Block;
use reth_rpc_types_compat::engine::payload::block_to_payload;
use std::time::Instant;
use tracing::{debug, info};

/// `reth benchmark new-payload-only` command
#[derive(Debug, Parser)]
pub struct Command {
    /// The RPC url to use for getting data.
    #[arg(long, value_name = "RPC_URL", verbatim_doc_comment)]
    rpc_url: String,

    #[command(flatten)]
    benchmark: BenchmarkArgs,
}

impl Command {
    /// Execute `benchmark new-payload-only` command
    pub async fn execute(self, _ctx: CliContext) -> eyre::Result<()> {
        let cloned_args = self.benchmark.clone();
        // TODO: this could be just a function I guess, but destructuring makes the code slightly
        // more readable than a 4 element tuple.
        let BenchContext { benchmark_mode, block_provider, auth_provider, mut next_block } =
            BenchContext::new(&cloned_args, self.rpc_url).await?;

        let (sender, mut receiver) = tokio::sync::mpsc::channel(1000);
        tokio::task::spawn(async move {
            while benchmark_mode.contains(next_block) {
                let block_res = block_provider.get_block_by_number(next_block.into(), true).await;
                let block = block_res.unwrap().unwrap();
                let block_hash = block.header.hash;
                let block = Block::try_from(block.inner).unwrap().seal(block_hash);

                next_block += 1;
                sender.send(block).await.unwrap();
            }
        });

        // put results in a summary vec so they can be printed at the end
        let mut results = Vec::new();
        let total_benchmark_duration = Instant::now();

        while let Some(block) = receiver.recv().await {
            // just put gas used here
            let gas_used = block.header.gas_used;

            let versioned_hashes: Vec<B256> =
                block.blob_versioned_hashes().into_iter().copied().collect();
            let parent_beacon_block_root = block.parent_beacon_block_root;
            let payload = block_to_payload(block);

            let block_number = payload.block_number();

            debug!(
                number=?payload.block_number(),
                "Sending payload to engine",
            );

            let start = Instant::now();
            call_new_payload(&auth_provider, payload, parent_beacon_block_root, versioned_hashes)
                .await?;

            let new_payload_result = NewPayloadResult { gas_used, latency: start.elapsed() };
            info!(%new_payload_result);

            // current duration since the start of the benchmark
            let current_duration = total_benchmark_duration.elapsed();

            // record the current result
            let row = TotalGasRow { block_number, gas_used, time: current_duration };
            results.push((row, new_payload_result));
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

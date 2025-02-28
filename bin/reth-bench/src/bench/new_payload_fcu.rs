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
use alloy_consensus::{Block, BlockBody, Transaction};
use alloy_primitives::B256;
use alloy_provider::{
    network::{AnyHeader, AnyRpcBlock},
    Provider,
};
use alloy_rpc_types::{BlockTransactions, Header as RpcHeader};
use alloy_rpc_types_engine::{ExecutionPayload, ForkchoiceState};
use clap::Parser;
use csv::Writer;
use reth_cli_runner::CliContext;
use reth_node_core::args::BenchmarkArgs;
use std::time::{Duration, Instant};
use tracing::{debug, info};

use super::rpc_transaction::RpcTransaction;

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
        let BenchContext { benchmark_mode, block_provider, auth_provider, mut next_block } =
            BenchContext::new(&self.benchmark, self.rpc_url).await?;

        let (sender, mut receiver) = tokio::sync::mpsc::channel(1000);
        tokio::task::spawn(async move {
            while benchmark_mode.contains(next_block) {
                let block_res =
                    block_provider.get_block_by_number(next_block.into(), true.into()).await;
                let block = block_res.unwrap().unwrap();
                let (header, versioned_hashes, payload) = from_any_rpc_block(block).unwrap();
                let head_block_hash = header.hash;
                let safe_block_hash = block_provider
                    .get_block_by_number(header.number.saturating_sub(32).into(), false.into());

                let finalized_block_hash = block_provider
                    .get_block_by_number(header.number.saturating_sub(64).into(), false.into());

                let (safe, finalized) = tokio::join!(safe_block_hash, finalized_block_hash,);

                let safe_block_hash = safe.unwrap().expect("finalized block exists").header.hash;
                let finalized_block_hash =
                    finalized.unwrap().expect("finalized block exists").header.hash;

                next_block += 1;
                sender
                    .send((
                        header,
                        versioned_hashes,
                        payload,
                        head_block_hash,
                        safe_block_hash,
                        finalized_block_hash,
                    ))
                    .await
                    .unwrap();
            }
        });

        // put results in a summary vec so they can be printed at the end
        let mut results = Vec::new();
        let total_benchmark_duration = Instant::now();
        let mut total_wait_time = Duration::ZERO;

        while let Some((header, versioned_hashes, payload, head, safe, finalized)) = {
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
            let message_version = call_new_payload(
                &auth_provider,
                payload,
                header.parent_beacon_block_root,
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

            // current duration since the start of the benchmark minus the time
            // waiting for blocks
            let current_duration = total_benchmark_duration.elapsed() - total_wait_time;

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

// TODO(mattsse): integrate in alloy
pub(crate) fn from_any_rpc_block(
    block: AnyRpcBlock,
) -> eyre::Result<(RpcHeader<AnyHeader>, Vec<B256>, ExecutionPayload)> {
    let block = block.inner.try_map_transactions(|tx| {
        // TODO: convert without json roundtrip
        serde_json::to_value(tx).and_then(serde_json::from_value::<RpcTransaction>)
    })?;

    let header = block.header.clone();

    // Extract transactions
    let transactions = match block.transactions {
        BlockTransactions::Hashes(_) => {
            return Err(eyre::eyre!("Block must include full transaction data. Send the eth_getBlockByHash request with full: `true`"));
        }
        BlockTransactions::Full(txs) => txs,
        BlockTransactions::Uncle => {
            return Err(eyre::eyre!("Cannot process uncle blocks"));
        }
    };

    // Extract blob versioned hashes
    let blob_versioned_hashes = transactions
        .iter()
        .filter_map(|tx| tx.blob_versioned_hashes().map(|v| v.to_vec()))
        .flatten()
        .collect::<Vec<_>>();
    let execution_payload = ExecutionPayload::from_block_unchecked(
        block.header.hash,
        &Block::new(
            block.header,
            BlockBody { transactions, ommers: vec![], withdrawals: block.withdrawals },
        ),
    )
    .0;

    Ok((header, blob_versioned_hashes, execution_payload))
}

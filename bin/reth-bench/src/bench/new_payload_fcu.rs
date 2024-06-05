//! Runs the `reth bench` command, calling first newPayload for each block, then calling
//! forkchoiceUpdated.

use crate::{
    authenticated_transport::AuthenticatedTransportConnect,
    bench::output::{
        CombinedResult, NewPayloadResult, TotalGasOutput, TotalGasRow, COMBINED_OUTPUT_SUFFIX,
        GAS_OUTPUT_SUFFIX,
    },
    bench_mode::BenchMode,
    valid_payload::EngineApiValidWaitExt,
};
use alloy_provider::{network::AnyNetwork, Provider, ProviderBuilder, RootProvider};
use alloy_rpc_client::ClientBuilder;
use alloy_rpc_types_engine::{ForkchoiceState, JwtSecret};
use clap::Parser;
use csv::Writer;
use reqwest::Url;
use reth_cli_runner::CliContext;
use reth_node_core::args::BenchmarkArgs;
use reth_primitives::{Block, B256};
use reth_rpc_types::BlockNumberOrTag;
use reth_rpc_types_compat::engine::payload::block_to_payload_v3;
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
        info!("Running benchmark using data from RPC URL: {}", self.rpc_url);

        // Ensure that output directory is a directory
        if let Some(output) = &self.benchmark.output {
            if output.is_file() {
                return Err(eyre::eyre!("Output path must be a directory"));
            }
        }

        // set up alloy client for blocks
        let block_provider = ProviderBuilder::new().on_http(self.rpc_url.parse()?);

        // If neither `--from` nor `--to` are provided, we will run the benchmark continuously,
        // starting at the latest block.
        let mut benchmark_mode = match (self.benchmark.from, self.benchmark.to) {
            (Some(from), Some(to)) => BenchMode::Range(from..=to),
            (None, None) => BenchMode::Continuous,
            _ => {
                // both or neither are allowed, everything else is ambiguous
                return Err(eyre::eyre!(
                    "Both --benchmark.from and --benchmark.to must be provided together"
                ))
            }
        };

        // construct the authenticated provider
        let auth_jwt = self.benchmark.auth_jwtsecret.ok_or_else(|| {
            eyre::eyre!("--auth-jwtsecret must be provided for authenticated RPC")
        })?;

        // fetch jwt from file
        //
        // the jwt is hex encoded so we will decode it after
        let jwt = std::fs::read_to_string(auth_jwt)?;
        let jwt = JwtSecret::from_hex(jwt)?;

        // get engine url
        let auth_url = Url::parse(&self.benchmark.engine_rpc_url)?;

        // construct the authed transport
        info!("Connecting to Engine RPC at {} for replay", auth_url);
        let auth_transport = AuthenticatedTransportConnect::new(auth_url, jwt);
        let client = ClientBuilder::default().connect_boxed(auth_transport).await?;
        let auth_provider = RootProvider::<_, AnyNetwork>::new(client);

        let first_block = match benchmark_mode {
            BenchMode::Continuous => {
                // fetch Latest block
                block_provider.get_block_by_number(BlockNumberOrTag::Latest, true).await?.unwrap()
            }
            BenchMode::Range(ref mut range) => {
                match range.next() {
                    Some(block_number) => {
                        // fetch first block in range
                        block_provider
                            .get_block_by_number(block_number.into(), true)
                            .await?
                            .unwrap()
                    }
                    None => {
                        // return an error
                        panic!("RangeEmpty");
                    }
                }
            }
        };

        let mut next_block = match first_block.header.number {
            Some(number) => {
                // fetch next block
                number + 1
            }
            None => {
                // this should never happen
                // TODO: log or return error, we should probably not return the
                // block here
                panic!("BlockNumberNone");
            }
        };

        let (sender, mut receiver) = tokio::sync::mpsc::channel(1000);
        tokio::task::spawn(async move {
            while benchmark_mode.contains(next_block) {
                let block_res = block_provider.get_block_by_number(next_block.into(), true).await;
                let block = block_res.unwrap().unwrap();
                let block = match block.header.hash {
                    Some(block_hash) => {
                        // we can reuse the hash in the response
                        Block::try_from(block).unwrap().seal(block_hash)
                    }
                    None => {
                        // we don't have the hash, so let's just hash it
                        Block::try_from(block).unwrap().seal_slow()
                    }
                };

                let head_block_hash = block.hash();
                let safe_block_hash =
                    block_provider.get_block_by_number((block.number - 32).into(), false);

                let finalized_block_hash =
                    block_provider.get_block_by_number((block.number - 64).into(), false);

                let (safe, finalized) = tokio::join!(safe_block_hash, finalized_block_hash,);

                let safe_block_hash = safe
                    .unwrap()
                    .expect("finalized block exists")
                    .header
                    .hash
                    .expect("finalized block has hash");
                let finalized_block_hash = finalized
                    .unwrap()
                    .expect("finalized block exists")
                    .header
                    .hash
                    .expect("finalized block has hash");

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

            let versioned_hashes: Vec<B256> =
                block.blob_versioned_hashes().into_iter().copied().collect();
            let (payload, parent_beacon_block_root) = block_to_payload_v3(block);
            let block_number = payload.payload_inner.payload_inner.block_number;

            debug!(
                ?block_number,
                hash=?payload.payload_inner.payload_inner.block_hash,
                parent_hash=?payload.payload_inner.payload_inner.parent_hash,
                "Sending payload",
            );

            let parent_beacon_block_root =
                parent_beacon_block_root.expect("this is a valid v3 payload");

            // construct fcu to call
            let forkchoice_state = ForkchoiceState {
                head_block_hash: head,
                safe_block_hash: safe,
                finalized_block_hash: finalized,
            };

            let start = Instant::now();
            auth_provider
                .new_payload_v3_wait(payload, versioned_hashes, parent_beacon_block_root)
                .await?;
            let new_payload_result = NewPayloadResult { gas_used, latency: start.elapsed() };

            auth_provider.fork_choice_updated_v3_wait(forkchoice_state, None).await?;

            // calculate the total duration and the fcu latency, record
            let total_latency = start.elapsed();
            let fcu_latency = total_latency - new_payload_result.latency;
            let combined_result = CombinedResult { new_payload_result, fcu_latency, total_latency };

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

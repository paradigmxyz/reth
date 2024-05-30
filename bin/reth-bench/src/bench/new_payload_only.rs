//! Runs the `reth bench` command, sending only newPayload, without a forkchoiceUpdated call.

use std::time::{Duration, Instant};

use crate::{
    authenticated_transport::AuthenticatedTransportConnect, bench_mode::BenchMode,
    valid_payload::EngineApiValidWaitExt,
};
use alloy_provider::{network::AnyNetwork, Provider, ProviderBuilder, RootProvider};
use alloy_rpc_client::ClientBuilder;
use alloy_rpc_types_engine::JwtSecret;
use clap::Parser;
use reqwest::Url;
use reth_cli_runner::CliContext;
use reth_node_core::args::BenchmarkArgs;
use reth_primitives::{Block, B256};
use reth_rpc_types::BlockNumberOrTag;
use reth_rpc_types_compat::engine::payload::block_to_payload_v3;
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
        info!("Running benchmark using data from RPC URL: {}", self.rpc_url);

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

                next_block += 1;
                sender.send(block).await.unwrap();
            }
        });

        // put results in a summary vec so they can be printed at the end
        // TODO: just accumulate on the fly
        let mut results = Vec::new();

        while let Some(block) = receiver.recv().await {
            // just put gas used here
            let gas_used = block.header.gas_used as f64;

            let versioned_hashes: Vec<B256> =
                block.blob_versioned_hashes().into_iter().copied().collect();
            let (payload, parent_beacon_block_root) = block_to_payload_v3(block);

            debug!(
                number=?payload.payload_inner.payload_inner.block_number,
                hash=?payload.payload_inner.payload_inner.block_hash,
                parent_hash=?payload.payload_inner.payload_inner.parent_hash,
                "Sending payload",
            );

            let parent_beacon_block_root =
                parent_beacon_block_root.expect("this is a valid v3 payload");
            let start = Instant::now();

            info!(?payload.payload_inner.payload_inner.block_number, "Sending payload to engine");
            auth_provider
                .new_payload_v3_wait(payload, versioned_hashes, parent_beacon_block_root)
                .await?;
            let new_payload_duration = start.elapsed();
            info!(?new_payload_duration, "Payload processed");

            // convert gas used to gigagas, then compute gigagas per second
            let gigagas_used = gas_used / 1_000_000_000.0;
            let gigagas_per_second = gigagas_used / new_payload_duration.as_secs_f64();
            results.push((new_payload_duration, gas_used));
            info!(
                ?new_payload_duration,
                "New payload processed at {:.2} Ggas/s, used {} total gas",
                gigagas_per_second,
                gas_used
            );
        }

        // accumulate the results and calculate the overall Ggas/s
        let total_gas_used: f64 = results.iter().map(|(_, gas)| gas).sum();
        let total_duration: Duration = results.iter().map(|(dur, _)| dur).sum();
        let total_ggas_used = total_gas_used / 1_000_000_000.0;
        let total_ggas_per_second = total_ggas_used / total_duration.as_secs_f64();
        info!(?total_duration, ?total_gas_used, "Total Ggas/s: {:.2}", total_ggas_per_second);

        // TODO: support properly sending versioned fork stuff. like if timestamp > fork, use
        // correct engine method
        Ok(())
    }
}

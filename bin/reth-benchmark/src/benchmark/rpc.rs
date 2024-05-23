//! Runs the `reth benchmark` using a remote rpc api.

use std::time::Instant;

use crate::{
    authenticated_transport::AuthenticatedTransportConnect, benchmark_mode::BenchmarkMode,
    block_fetcher::BlockStream, valid_payload::EngineApiValidWaitExt,
};
use alloy_provider::{network::AnyNetwork, Provider, ProviderBuilder, RootProvider};
use alloy_rpc_client::ClientBuilder;
use alloy_rpc_types_engine::{ForkchoiceState, JwtSecret};
use clap::Parser;
use futures::StreamExt;
use reqwest::Url;
use reth_cli_runner::CliContext;
use reth_node_core::args::BenchmarkArgs;
use reth_primitives::Block;
use reth_rpc_types_compat::engine::payload::block_to_payload_v3;
use tracing::info;

/// `reth benchmark from-rpc` command
#[derive(Debug, Parser)]
pub struct Command {
    /// The RPC url to use for getting data.
    #[arg(long, value_name = "RPC_URL", verbatim_doc_comment)]
    rpc_url: String,

    #[command(flatten)]
    benchmark: BenchmarkArgs,
}

impl Command {
    /// Execute `benchmark from-rpc` command
    pub async fn execute(self, _ctx: CliContext) -> eyre::Result<()> {
        info!("Running benchmark using data from RPC URL: {}", self.rpc_url);

        // set up alloy client for blocks
        let block_provider = ProviderBuilder::new().on_http(self.rpc_url.parse()?);

        // If neither `--from` nor `--to` are provided, we will run the benchmark continuously,
        // starting at the latest block.
        let benchmark_mode = match (self.benchmark.from, self.benchmark.to) {
            (Some(from), Some(to)) => BenchmarkMode::Range(from..=to),
            (None, None) => BenchmarkMode::Continuous,
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

        // construct the stream
        let mut block_stream = BlockStream::new(benchmark_mode, &block_provider, 10)?;

        while let Some(block_res) = block_stream.next().await {
            let block = block_res?.ok_or(BlockResponseError::BlockStreamNone)?;
            let block = match block.header.hash {
                Some(block_hash) => {
                    // we can reuse the hash in the response
                    Block::try_from(block)?.seal(block_hash)
                }
                None => {
                    // we don't have the hash, so let's just hash it
                    Block::try_from(block)?.seal_slow()
                }
            };

            // see the todo in the block fetcher for ways this can be improved
            let head_block_hash = block.hash();
            let safe_block_hash = block_provider
                .get_block_by_number((block.number - 32).into(), false)
                .await?
                .expect("safe block exists")
                .header
                .hash
                .expect("safe block has hash");

            let finalized_block_hash = block_provider
                .get_block_by_number((block.number - 64).into(), false)
                .await?
                .expect("finalized block exists")
                .header
                .hash
                .expect("finalized block has hash");

            // just put gas used here
            let gas_used = block.header.gas_used as f64;

            let versioned_hashes = block.blob_versioned_hashes().into_iter().copied().collect();
            let (payload, parent_beacon_block_root) = block_to_payload_v3(block);

            info!(
                number=?payload.payload_inner.payload_inner.block_number,
                hash=?payload.payload_inner.payload_inner.block_hash,
                parent_hash=?payload.payload_inner.payload_inner.parent_hash,
                "Sending payload",
            );

            let start = Instant::now();
            auth_provider
                .new_payload_v3_wait(
                    payload,
                    versioned_hashes,
                    parent_beacon_block_root.expect("this is a valid v3 payload"),
                )
                .await?;
            let new_payload_duration = start.elapsed();

            // construct fcu to call
            let forkchoice_state =
                ForkchoiceState { head_block_hash, safe_block_hash, finalized_block_hash };

            // TODO: allow the user to configure what to do when they get the output of the stream,
            // ie, the block inside of this while loop should be configurable, along with the stream
            // itself
            auth_provider.fork_choice_updated_v3_wait(forkchoice_state, None).await?;
            let fcu_duration = start.elapsed() - new_payload_duration;

            // convert gas used to gigagas, then compute gigagas per second
            let gigagas_used = gas_used / 1_000_000_000.0;
            let gigagas_per_second = gigagas_used / new_payload_duration.as_secs_f64();
            info!(
                ?new_payload_duration,
                "New payload processed at {:.2} Ggas/s, used {} total gas",
                gigagas_per_second,
                gas_used
            );
            info!(?fcu_duration, "fcu duration");
        }

        // TODO: support properly sending versioned fork stuff. like if timestamp > fork, use
        // correct engine method
        Ok(())
    }
}

/// An error that can occur when trying to convert or fetch a block.
#[derive(Debug, thiserror::Error)]
pub(crate) enum BlockResponseError {
    /// The block stream returned a `None` value, meaning the block range provided may be invalid.
    #[error(
        "Block stream returned a None value, meaning the block range provided may be invalid."
    )]
    BlockStreamNone,
}

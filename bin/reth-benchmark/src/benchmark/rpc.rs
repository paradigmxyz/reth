//! Runs the `reth benchmark` using a remote rpc api.

use crate::{
    authenticated_transport::AuthenticatedTransportConnect, benchmark_mode::BenchmarkMode,
    block_fetcher::BlockStream, valid_payload::EngineApiValidWaitExt,
};
use alloy_provider::{network::AnyNetwork, ProviderBuilder, RootProvider};
use alloy_rpc_client::ClientBuilder;
use alloy_rpc_types_engine::{ExecutionPayloadInputV2, JwtSecret};
use clap::Parser;
use futures::StreamExt;
use reqwest::Url;
use reth_cli_runner::CliContext;
use reth_node_core::args::BenchmarkArgs;
use reth_primitives::Block;
use reth_rpc_types_compat::engine::payload::block_to_payload_v2;
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
        // TODO: set up alloy client for non engine rpc url
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

            let payload = block_to_payload_v2(block);
            let payload = ExecutionPayloadInputV2 {
                execution_payload: payload.payload_inner,
                withdrawals: Some(payload.withdrawals),
            };
            println!(
                "number: {:?}, hash: {:?}, parent_hash: {:?}",
                payload.execution_payload.block_number,
                payload.execution_payload.block_hash,
                payload.execution_payload.parent_hash
            );
            auth_provider.new_payload_v2_wait(payload).await?;
        }

        // TODO: make `Continuous` work properly, or remove it

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

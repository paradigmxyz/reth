//! Runs the `reth benchmark` using a remote rpc api.

use crate::{benchmark_mode::BenchmarkMode, block_fetcher::BlockStream};
use alloy_provider::{Provider, ProviderBuilder};
use clap::Parser;
use futures::StreamExt;
use reth_cli_runner::CliContext;
use reth_node_core::args::BenchmarkArgs;
use reth_primitives::Block;
use reth_rpc_types_compat::engine::payload::block_to_payload_v3;

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
    pub async fn execute(self, ctx: CliContext) -> eyre::Result<()> {
        // TODO: set up alloy client for non engine rpc url
        // TODO: use ws only
        let block_provider = ProviderBuilder::new().on_http(self.rpc_url.parse()?);

        // TODO: construct this from args
        let benchmark_mode = BenchmarkMode::Range(19_000_000..=20_000_000);

        let mut block_stream = BlockStream::new(benchmark_mode, &block_provider, 10)?;

        while let Some(block_res) = block_stream.next().await {
            let block = block_res?.expect("block stream should not return None blocks");
            let block_hash = block.header.hash.expect("block hash");
            let block = Block::try_from(block)?.seal(block_hash);
            let payload = block_to_payload_v3(block);
            println!("{:?}", payload);
        }

        // TODO: support properly sending versioned fork stuff. like if timestamp > fork, use
        // correct engine method

        // TODO: reusable method for payload stream + rpc url + benchmark config -> run the
        // benchmark
        Ok(())
    }
}

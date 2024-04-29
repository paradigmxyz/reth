//! Runs the `reth benchmark` using a remote rpc api.

use alloy_provider::{Provider, ProviderBuilder};
use clap::Parser;
use reth_cli_runner::CliContext;
use reth_node_core::args::BenchmarkArgs;
use reth_primitives::Block;
use reth_rpc_types_compat::engine::payload::block_to_payload_v3;

use crate::payload_stream::BenchmarkMode;

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

        // TODO: stream of _full_ blocks either from a range or from the latest block
        for block in 19_000_000..20_000_000 {
            let block = block_provider
                .get_block_by_number(block.into(), true)
                .await?
                .expect("remove all of this code");

            // let primitive_block = Block::try_from(block)?;
            // block_to_payload_v3(value)
        }

        // TODO: support properly sending versioned fork stuff. like if timestamp > fork, use
        // correct engine method

        // TODO: reusable method for payload stream + rpc url + benchmark config -> run the
        // benchmark
        Ok(())
    }
}

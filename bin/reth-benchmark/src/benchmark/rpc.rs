//! Runs the `reth benchmark` using a remote rpc api.

use crate::{benchmark_mode::BenchmarkMode, block_fetcher::BlockStream};
use alloy_provider::ProviderBuilder;
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

            let payload = block_to_payload_v3(block);
            println!(
                "number: {:?}, hash: {:?}, parent_hash: {:?}",
                payload.payload_inner.payload_inner.block_number,
                payload.payload_inner.payload_inner.block_hash,
                payload.payload_inner.payload_inner.parent_hash
            );
        }

        // TODO: support properly sending versioned fork stuff. like if timestamp > fork, use
        // correct engine method

        // TODO: reusable method for payload stream + rpc url + benchmark config -> run the
        // benchmark
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

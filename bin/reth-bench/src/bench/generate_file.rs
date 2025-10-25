use crate::bench::{context::BenchContext, output::BLOCK_STORAGE_OUTPUT_SUFFIX};
use alloy_provider::Provider;
use alloy_rlp::Encodable;
use clap::Parser;
use eyre::OptionExt;
use reth_cli_runner::CliContext;
use reth_node_core::args::BenchmarkArgs;
use std::{
    fs::File,
    io::{BufWriter, Write},
};
use tracing::info;

/// `reth benchmark generate-file` command
#[derive(Debug, Parser)]
pub struct Command {
    /// The RPC url to use for getting data.
    #[arg(long, value_name = "RPC_URL", verbatim_doc_comment)]
    rpc_url: String,

    // #[arg(long, value_name = "OUTPUT", verbatim_doc_comment)]
    // output: PathBuf,
    #[command(flatten)]
    benchmark: BenchmarkArgs,
}

impl Command {
    /// Execute `benchmark generate-file` command
    pub async fn execute(self, _ctx: CliContext) -> eyre::Result<()> {
        info!("Generating file from RPC: {}", self.rpc_url);

        let BenchContext { block_provider, benchmark_mode, mut next_block, is_optimism: _, .. } =
            BenchContext::new(&self.benchmark, Some(self.rpc_url)).await?;

        // Open file once
        let output_path = self
            .benchmark
            .output
            .ok_or_eyre("--output is required")?
            .join(BLOCK_STORAGE_OUTPUT_SUFFIX);
        let mut writer = BufWriter::new(File::create(output_path)?);

        // Simple loop - fetch and write
        while benchmark_mode.contains(next_block) {
            info!("Fetching block {}", next_block);

            // Fetch block
            let block = block_provider
                .as_ref()
                .unwrap()
                .get_block_by_number(next_block.into())
                .full()
                .await?
                .ok_or_eyre("Block not found")?;

            // Convert to consensus
            let consensus_block = block
                .into_inner()
                .map_header(|header| header.map(|h| h.into_header_with_defaults()))
                .try_map_transactions(|tx| {
                    tx.try_into_either::<op_alloy_consensus::OpTxEnvelope>()
                })?
                .into_consensus();

            // Encode
            let mut rlp = Vec::with_capacity(consensus_block.length());
            consensus_block.encode(&mut rlp);

            // Write length prefix + data + newline
            writer.write_all(&(rlp.len() as u32).to_le_bytes())?;
            writer.write_all(&rlp)?;
            writer.write_all(b"\n")?;

            next_block += 1;
        }

        writer.flush()?;
        info!("Finished writing blocks to file");

        Ok(())
    }
}

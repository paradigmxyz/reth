//! Command that imports RLP encoded blocks from a file, similar to go-ethereum's ImportChain.
//!
//! This implementation closely follows the Go ImportChain function:
//! - Reads RLP-encoded blocks from a file (supports gzip)
//! - Imports blocks in batches
//! - Skips genesis block (block 0)
//! - Only imports blocks that are missing from the database
//! - Handles interrupts gracefully (Ctrl+C)

use clap::Parser;
use eyre::{eyre, Result};
use reth_chainspec::ChainSpecProvider;
use reth_cli::chainspec::ChainSpecParser;
use reth_cli_commands::{
    common::{AccessRights, CliNodeComponents, CliNodeTypes, Environment, EnvironmentArgs},
    import_core::{import_blocks_from_file, ImportConfig},
};
use reth_node_core::version::version_metadata;
use reth_optimism_chainspec::OpChainSpec;
use std::{path::PathBuf, sync::Arc};
use tracing::info;

/// Syncs RLP encoded blocks from a file, similar to go-ethereum's import command.
#[derive(Debug, Parser)]
pub struct ImportCommand<C: ChainSpecParser> {
    #[command(flatten)]
    env: EnvironmentArgs<C>,

    /// Disables stages that require state.
    #[arg(long, verbatim_doc_comment)]
    no_state: bool,

    /// Chunk byte length to read from file.
    #[arg(long, value_name = "CHUNK_LEN", verbatim_doc_comment)]
    chunk_len: Option<u64>,

    /// The path to a block file for import.
    ///
    /// Blocks should be RLP encoded. The file can be gzip compressed if it ends with .gz
    #[arg(value_name = "IMPORT_PATH", verbatim_doc_comment)]
    path: PathBuf,
}

impl<C: ChainSpecParser<ChainSpec = OpChainSpec>> ImportCommand<C> {
    /// Execute `import` command
    pub async fn execute<N, Comp>(
        self,
        components: impl FnOnce(Arc<N::ChainSpec>) -> Comp,
    ) -> Result<()>
    where
        N: CliNodeTypes<ChainSpec = C::ChainSpec>,
        Comp: CliNodeComponents<N>,
    {
        info!(target: "reth::cli", "reth {} starting", version_metadata().short_version);
        info!(target: "reth::cli", "Importing blockchain from file: {}", self.path.display());

        let Environment { provider_factory, config, .. } = self.env.init::<N>(AccessRights::RW)?;

        let components = components(provider_factory.chain_spec());

        let import_config = ImportConfig { no_state: self.no_state, chunk_len: self.chunk_len };

        let executor = components.evm_config().clone();
        let consensus = Arc::new(components.consensus().clone());

        let result = import_blocks_from_file(
            &self.path,
            import_config,
            provider_factory,
            &config,
            executor,
            consensus,
        )
        .await?;

        if !result.is_complete() {
            return Err(eyre!(
                "Chain was partially imported from file: {}. Imported {}/{} blocks, {}/{} transactions",
                self.path.display(),
                result.total_imported_blocks,
                result.total_decoded_blocks,
                result.total_imported_txns,
                result.total_decoded_txns
            ));
        }

        info!(target: "reth::cli",
            "Import complete! Imported {}/{} blocks, {}/{} transactions",
            result.total_imported_blocks,
            result.total_decoded_blocks,
            result.total_imported_txns,
            result.total_decoded_txns
        );

        Ok(())
    }
}

impl<C: ChainSpecParser> ImportCommand<C> {
    /// Returns the underlying chain being used to run this command
    pub fn chain_spec(&self) -> Option<&Arc<C::ChainSpec>> {
        Some(&self.env.chain)
    }
}


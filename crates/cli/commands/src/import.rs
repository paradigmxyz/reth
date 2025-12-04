//! Command that initializes the node by importing a chain from a file.
use crate::{
    common::{AccessRights, CliNodeComponents, CliNodeTypes, Environment, EnvironmentArgs},
    import_core::{import_blocks_from_file, ImportConfig},
};
use clap::Parser;
use reth_chainspec::{ChainSpecProvider, EthChainSpec, EthereumHardforks};
use reth_cli::chainspec::ChainSpecParser;
use reth_node_core::version::version_metadata;
use std::{path::PathBuf, sync::Arc};
use tracing::info;

pub use crate::import_core::build_import_pipeline_impl as build_import_pipeline;

/// Syncs RLP encoded blocks from a file or files.
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

    /// The path(s) to block file(s) for import.
    ///
    /// The online stages (headers and bodies) are replaced by a file import, after which the
    /// remaining stages are executed. Multiple files will be imported sequentially.
    #[arg(value_name = "IMPORT_PATH", required = true, num_args = 1.., verbatim_doc_comment)]
    paths: Vec<PathBuf>,
}

impl<C: ChainSpecParser<ChainSpec: EthChainSpec + EthereumHardforks>> ImportCommand<C> {
    /// Execute `import` command
    pub async fn execute<N, Comp>(
        self,
        components: impl FnOnce(Arc<N::ChainSpec>) -> Comp,
    ) -> eyre::Result<()>
    where
        N: CliNodeTypes<ChainSpec = C::ChainSpec>,
        Comp: CliNodeComponents<N>,
    {
        info!(target: "reth::cli", "reth {} starting", version_metadata().short_version);

        let Environment { provider_factory, config, .. } = self.env.init::<N>(AccessRights::RW)?;

        let components = components(provider_factory.chain_spec());

        info!(target: "reth::cli", "Starting import of {} file(s)", self.paths.len());

        let import_config = ImportConfig { no_state: self.no_state, chunk_len: self.chunk_len };

        let executor = components.evm_config().clone();
        let consensus = Arc::new(components.consensus().clone());

        let mut total_imported_blocks = 0;
        let mut total_imported_txns = 0;
        let mut total_decoded_blocks = 0;
        let mut total_decoded_txns = 0;

        // Import each file sequentially
        for (index, path) in self.paths.iter().enumerate() {
            info!(target: "reth::cli", "Importing file {} of {}: {}", index + 1, self.paths.len(), path.display());

            let result = import_blocks_from_file(
                path,
                import_config.clone(),
                provider_factory.clone(),
                &config,
                executor.clone(),
                consensus.clone(),
            )
            .await?;

            total_imported_blocks += result.total_imported_blocks;
            total_imported_txns += result.total_imported_txns;
            total_decoded_blocks += result.total_decoded_blocks;
            total_decoded_txns += result.total_decoded_txns;

            if !result.is_complete() {
                return Err(eyre::eyre!(
                    "Chain was partially imported from file: {}. Imported {}/{} blocks, {}/{} transactions",
                    path.display(),
                    result.total_imported_blocks,
                    result.total_decoded_blocks,
                    result.total_imported_txns,
                    result.total_decoded_txns
                ));
            }

            info!(target: "reth::cli",
                  "Successfully imported file {}: {} blocks, {} transactions",
                  path.display(), result.total_imported_blocks, result.total_imported_txns);
        }

        info!(target: "reth::cli",
              "All files imported successfully. Total: {}/{} blocks, {}/{} transactions",
              total_imported_blocks, total_decoded_blocks, total_imported_txns, total_decoded_txns);

        Ok(())
    }
}

impl<C: ChainSpecParser> ImportCommand<C> {
    /// Returns the underlying chain being used to run this command
    pub fn chain_spec(&self) -> Option<&Arc<C::ChainSpec>> {
        Some(&self.env.chain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_ethereum_cli::chainspec::{EthereumChainSpecParser, SUPPORTED_CHAINS};

    #[test]
    fn parse_common_import_command_chain_args() {
        for chain in SUPPORTED_CHAINS {
            let args: ImportCommand<EthereumChainSpecParser> =
                ImportCommand::parse_from(["reth", "--chain", chain, "."]);
            assert_eq!(
                Ok(args.env.chain.chain),
                chain.parse::<reth_chainspec::Chain>(),
                "failed to parse chain {chain}"
            );
        }
    }

    #[test]
    fn parse_import_command_with_multiple_paths() {
        let args: ImportCommand<EthereumChainSpecParser> =
            ImportCommand::parse_from(["reth", "file1.rlp", "file2.rlp", "file3.rlp"]);
        assert_eq!(args.paths.len(), 3);
        assert_eq!(args.paths[0], PathBuf::from("file1.rlp"));
        assert_eq!(args.paths[1], PathBuf::from("file2.rlp"));
        assert_eq!(args.paths[2], PathBuf::from("file3.rlp"));
    }
}

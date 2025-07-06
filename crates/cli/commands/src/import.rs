//! Command that initializes the node by importing a chain from a file.
use crate::{
    common::{AccessRights, CliNodeComponents, CliNodeTypes, Environment, EnvironmentArgs},
    import_op::{import_blocks_from_file, ImportConfig},
};
use clap::Parser;
use reth_chainspec::{ChainSpecProvider, EthChainSpec, EthereumHardforks};
use reth_cli::chainspec::ChainSpecParser;
use reth_node_core::version::SHORT_VERSION;
use std::{path::PathBuf, sync::Arc};
use tracing::info;

pub use crate::import_op::build_import_pipeline_impl as build_import_pipeline;

/// Syncs RLP encoded blocks from a file.
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
    /// The online stages (headers and bodies) are replaced by a file import, after which the
    /// remaining stages are executed.
    #[arg(value_name = "IMPORT_PATH", verbatim_doc_comment)]
    path: PathBuf,
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
        info!(target: "reth::cli", "reth {} starting", SHORT_VERSION);

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
            return Err(eyre::eyre!("Chain was partially imported"));
        }

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
}

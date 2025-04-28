//! Command that initializes the node by importing a chain from a file.
use crate::common::{AccessRights, CliNodeTypes, Environment, EnvironmentArgs};
use clap::Parser;
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_cli::chainspec::ChainSpecParser;
use reth_era_downloader::read_dir;
use reth_etl::Collector;
use reth_node_core::version::SHORT_VERSION;
use std::{path::PathBuf, sync::Arc};
use tracing::info;

/// Syncs ERA encoded blocks from a directory.
#[derive(Debug, Parser)]
pub struct ImportEraCommand<C: ChainSpecParser> {
    #[command(flatten)]
    env: EnvironmentArgs<C>,

    /// The path to a directory for import.
    ///
    /// The ERA1 files are read from the directory parsing headers and bodies.
    #[arg(value_name = "IMPORT_PATH", verbatim_doc_comment)]
    path: PathBuf,
}

impl<C: ChainSpecParser<ChainSpec: EthChainSpec + EthereumHardforks>> ImportEraCommand<C> {
    /// Execute `import-era` command
    pub async fn execute<N>(self) -> eyre::Result<()>
    where
        N: CliNodeTypes<ChainSpec = C::ChainSpec>,
    {
        info!(target: "reth::cli", "reth {} starting", SHORT_VERSION);

        let Environment { provider_factory, config, .. } = self.env.init::<N>(AccessRights::RW)?;

        let hash_collector = Collector::new(config.stages.etl.file_size, config.stages.etl.dir);
        let stream = read_dir(self.path)?;

        reth_era_utils::import(stream, &provider_factory.provider_rw().unwrap().0, hash_collector)?;

        Ok(())
    }

    /// Returns the underlying chain being used to run this command
    pub fn chain_spec(&self) -> Option<&Arc<C::ChainSpec>> {
        Some(&self.env.chain)
    }
}

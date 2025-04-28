//! Command that initializes the node by importing a chain from a file.
use crate::common::{AccessRights, CliNodeTypes, Environment, EnvironmentArgs};
use clap::{Parser, Subcommand};
use reqwest::{Client, Url};
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_cli::chainspec::ChainSpecParser;
use reth_era_downloader::{read_dir, EraClient, EraStream, EraStreamConfig};
use reth_etl::Collector;
use reth_node_core::version::SHORT_VERSION;
use std::{path::PathBuf, sync::Arc};
use tracing::info;

/// Syncs ERA encoded blocks from a local or remote source.
#[derive(Debug, Parser)]
pub struct ImportEraCommand<C: ChainSpecParser> {
    #[command(flatten)]
    env: EnvironmentArgs<C>,

    #[command(subcommand)]
    import: ImportArgs<C>,
}

#[derive(Debug, Subcommand)]
pub enum ImportArgs<C: ChainSpecParser> {
    /// Read ERA1 files from a local directory.
    Local {
        /// The path to a directory for import.
        ///
        /// The ERA1 files are read from the local directory parsing headers and bodies.
        #[arg(value_name = "IMPORT_PATH", verbatim_doc_comment)]
        path: PathBuf,
    },
    /// Read ERA1 files from a remote URL.
    Remote {
        /// The URL to a remote host where the ERA1 files are hosted.
        ///
        /// The ERA1 files are read from the remote host using HTTP GET requests parsing headers
        /// and bodies.
        #[arg(value_name = "IMPORT_URL", verbatim_doc_comment)]
        url: Url,

        /// The chain this node is running.
        ///
        /// Possible values are either a built-in chain or the path to a chain specification file.
        #[arg(
            long,
            value_name = "CHAIN_OR_PATH",
            long_help = C::help_message(),
            default_value = C::SUPPORTED_CHAINS[0],
            value_parser = C::parser(),
            global = true
        )]
        chain: Arc<C::ChainSpec>,
    },
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

        match self.import {
            ImportArgs::Local { path } => {
                let stream = read_dir(path)?;

                reth_era_import::import(
                    stream,
                    &provider_factory.provider_rw().unwrap().0,
                    hash_collector,
                )?;
            }
            ImportArgs::Remote { url, .. } => {
                let folder = tempfile::tempdir()?;
                let folder = folder.path().to_owned().into_boxed_path();
                let client = EraClient::new(Client::new(), url, folder);
                let stream = EraStream::new(client, EraStreamConfig::default());

                reth_era_utils::import(
                    stream,
                    &provider_factory.provider_rw().unwrap().0,
                    hash_collector
                )?;
            }
        }

        Ok(())
    }

    /// Returns the underlying chain being used to run this command
    pub fn chain_spec(&self) -> Option<&Arc<C::ChainSpec>> {
        Some(&self.env.chain)
    }
}

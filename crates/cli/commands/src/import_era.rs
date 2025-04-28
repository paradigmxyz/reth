//! Command that initializes the node by importing a chain from ERA files.
use crate::common::{AccessRights, CliNodeTypes, Environment, EnvironmentArgs};
use clap::{Args, Parser, ValueEnum};
use eyre::OptionExt;
use reqwest::{Client, Url};
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_cli::chainspec::ChainSpecParser;
use reth_era_downloader::{read_dir, EraClient, EraStream, EraStreamConfig};
use reth_era_utils as era;
use reth_etl::Collector;
use reth_node_core::{dirs::data_dir, version::SHORT_VERSION};
use std::{path::PathBuf, sync::Arc};
use tracing::info;

/// Syncs ERA encoded blocks from a local or remote source.
#[derive(Debug, Parser)]
pub struct ImportEraCommand<C: ChainSpecParser> {
    #[command(flatten)]
    env: EnvironmentArgs<C>,

    #[clap(flatten)]
    import: ImportArgs,
}

#[derive(Debug, Args)]
#[group(required = true, multiple = false)]
pub struct ImportArgs {
    /// The path to a directory for import.
    ///
    /// The ERA1 files are read from the local directory parsing headers and bodies.
    #[arg(value_name = "IMPORT_ERA_PATH", verbatim_doc_comment)]
    path: Option<PathBuf>,

    /// The URL to a remote host where the ERA1 files are hosted.
    ///
    /// The ERA1 files are read from the remote host using HTTP GET requests parsing headers
    /// and bodies.
    #[arg(value_name = "IMPORT_ERA_URL", verbatim_doc_comment)]
    url: Option<Url>,

    /// The chain for which a known URL exists.
    ///
    /// When specified, the URL is derived from the chain name and read from the remote host using
    /// HTTP GET requests parsing headers and bodies.
    #[arg(value_enum, value_name = "IMPORT_ERA_CHAIN", verbatim_doc_comment)]
    chain_id: Option<Chain>,
}

/// An identifier of a chain for which a known URL that hosts ERA1 files exists.
///
/// The conversion into [`Url`] is done through [`Url::from`].
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum)]
pub enum Chain {
    Mainnet,
    Sepolia,
}

impl From<Chain> for Url {
    fn from(value: Chain) -> Self {
        match value {
            Chain::Mainnet => {
                Url::parse("https://mainnet.era1.nimbus.team/").expect("URL should be valid")
            }
            Chain::Sepolia => {
                Url::parse("https://sepolia.era1.nimbus.team/").expect("URL should be valid")
            }
        }
    }
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
        let provider_factory = &provider_factory.provider_rw()?.0;

        if let Some(path) = self.import.path {
            let stream = read_dir(path)?;

            era::import(stream, provider_factory, hash_collector)?;
        } else if let Some(url) = self.import.url.or(self.import.chain_id.map(Url::from)) {
            let folder = data_dir().ok_or_eyre("Missing data directory")?.join("era");
            let folder = folder.into_boxed_path();
            let client = EraClient::new(Client::new(), url, folder);
            let stream = EraStream::new(client, EraStreamConfig::default());

            era::import(stream, provider_factory, hash_collector)?;
        }

        Ok(())
    }

    /// Returns the underlying chain being used to run this command
    pub fn chain_spec(&self) -> Option<&Arc<C::ChainSpec>> {
        Some(&self.env.chain)
    }
}

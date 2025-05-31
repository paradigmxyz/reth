//! Command that initializes the node by importing a chain from ERA files.
use crate::common::{AccessRights, CliNodeTypes, Environment, EnvironmentArgs};
use alloy_chains::{ChainKind, NamedChain};
use clap::{Args, Parser};
use eyre::{eyre, OptionExt};
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
#[group(required = false, multiple = false)]
pub struct ImportArgs {
    /// The path to a directory for import.
    ///
    /// The ERA1 files are read from the local directory parsing headers and bodies.
    #[arg(long, value_name = "IMPORT_ERA_PATH", verbatim_doc_comment)]
    path: Option<PathBuf>,

    /// The URL to a remote host where the ERA1 files are hosted.
    ///
    /// The ERA1 files are read from the remote host using HTTP GET requests parsing headers
    /// and bodies.
    #[arg(long, value_name = "IMPORT_ERA_URL", verbatim_doc_comment)]
    url: Option<Url>,
}

trait TryFromChain {
    fn try_to_url(&self) -> eyre::Result<Url>;
}

impl TryFromChain for ChainKind {
    fn try_to_url(&self) -> eyre::Result<Url> {
        Ok(match self {
            ChainKind::Named(NamedChain::Mainnet) => {
                Url::parse("https://era.ithaca.xyz/era1/index.html").expect("URL should be valid")
            }
            ChainKind::Named(NamedChain::Sepolia) => {
                Url::parse("https://era.ithaca.xyz/sepolia-era1/index.html")
                    .expect("URL should be valid")
            }
            chain => return Err(eyre!("No known host for ERA files on chain {chain:?}")),
        })
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
        } else {
            let url = match self.import.url {
                Some(url) => url,
                None => self.env.chain.chain().kind().try_to_url()?,
            };
            let folder = data_dir().ok_or_eyre("Missing data directory")?.join("era");
            let folder = folder.into_boxed_path();
            let client = EraClient::new(Client::new(), url, folder);
            let stream = EraStream::new(client, EraStreamConfig::default());

            era::import(stream, provider_factory, hash_collector)?;
        }

        Ok(())
    }
}

impl<C: ChainSpecParser> ImportEraCommand<C> {
    /// Returns the underlying chain being used to run this command
    pub fn chain_spec(&self) -> Option<&Arc<C::ChainSpec>> {
        Some(&self.env.chain)
    }
}

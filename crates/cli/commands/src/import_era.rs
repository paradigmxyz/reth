//! Command that initializes the node by importing a chain from a file.
use crate::common::{AccessRights, CliNodeTypes, Environment, EnvironmentArgs};
use clap::Parser;
use reqwest::Url;
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_cli::chainspec::ChainSpecParser;
use reth_downloaders::file_client::DEFAULT_BYTE_LEN_CHUNK_CHAIN_FILE;
use reth_era_downloader::{EraClient, EraStream, EraStreamConfig};
use reth_etl::Collector;
use reth_node_core::version::SHORT_VERSION;
use std::{path::PathBuf, sync::Arc};
use tracing::{debug, info};

/// Syncs RLP encoded blocks from a file.
#[derive(Debug, Parser)]
pub struct ImportEraCommand<C: ChainSpecParser> {
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

    /// The base URL for the ERA hosting service.
    #[arg(value_name = "ERA_URL", verbatim_doc_comment)]
    url: Url,
}

impl<C: ChainSpecParser<ChainSpec: EthChainSpec + EthereumHardforks>> ImportEraCommand<C> {
    /// Execute `import-era` command
    pub async fn execute<N>(self) -> eyre::Result<()>
    where
        N: CliNodeTypes<ChainSpec = C::ChainSpec>,
    {
        info!(target: "reth::cli", "reth {} starting", SHORT_VERSION);

        if self.no_state {
            info!(target: "reth::cli", "Disabled stages requiring state");
        }

        debug!(target: "reth::cli",
            chunk_byte_len=self.chunk_len.unwrap_or(DEFAULT_BYTE_LEN_CHUNK_CHAIN_FILE),
            "Chunking chain import"
        );

        let Environment { provider_factory, config, .. } = self.env.init::<N>(AccessRights::RW)?;

        let folder = self.path;
        let client =
            EraClient::new(reqwest::Client::new(), self.url, folder.clone().into_boxed_path());

        client.fetch_file_list().await?;

        let hash_collector = Collector::new(config.stages.etl.file_size, config.stages.etl.dir);
        let config = EraStreamConfig::default().with_max_files(1).with_max_concurrent_downloads(1);
        let stream = EraStream::new(client, config);

        reth_era_import::import(
            stream,
            &provider_factory.provider_rw().unwrap().0,
            hash_collector,
        )?;

        Ok(())
    }

    /// Returns the underlying chain being used to run this command
    pub fn chain_spec(&self) -> Option<&Arc<C::ChainSpec>> {
        Some(&self.env.chain)
    }
}

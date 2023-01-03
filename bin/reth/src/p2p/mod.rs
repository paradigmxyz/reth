//! P2P Debugging tool
use clap::{Parser, Subcommand};
use futures::TryStreamExt;
use reth_consensus::BeaconConsensus;
use reth_db::mdbx::{Env, EnvKind, WriteMap};
use reth_downloaders::headers;
use reth_interfaces::p2p::headers::downloader::HeaderDownloader;
use reth_primitives::{Header, SealedHeader, H256};
use std::sync::Arc;

use crate::{
    config::Config,
    dirs::ConfigPath,
    util::chainspec::{chain_spec_value_parser, ChainSpecification},
};

/// `reth p2p` command
#[derive(Debug, Parser)]
pub struct Command {
    /// The path to the configuration file to use.
    #[arg(long, value_name = "FILE", verbatim_doc_comment, default_value_t)]
    config: ConfigPath,

    /// The chain this node is running.
    ///
    /// Possible values are either a built-in chain or the path to a chain specification file.
    ///
    /// Built-in chains:
    /// - mainnet
    /// - goerli
    /// - sepolia
    #[arg(
        long,
        value_name = "CHAIN_OR_PATH",
        verbatim_doc_comment,
        default_value = "mainnet",
        value_parser = chain_spec_value_parser
    )]
    chain: ChainSpecification,

    /// Disable the discovery service.
    #[arg(short, long)]
    disable_discovery: bool,

    #[clap(subcommand)]
    command: Subcommands,
}

#[derive(Subcommand, Debug)]
/// `reth p2p` subcommands
pub enum Subcommands {
    /// Download block header
    Header {
        /// The header hash
        hash: H256,
    },
}
impl Command {
    /// Execute `p2p` command
    pub async fn execute(&self) -> eyre::Result<()> {
        let tempdir = tempfile::TempDir::new()?;
        let noop_db = Arc::new(Env::<WriteMap>::open(&tempdir.into_path(), EnvKind::RW)?);

        let config: Config = confy::load_path(&self.config).unwrap_or_default();

        let chain_id = self.chain.consensus.chain_id;
        let genesis: Header = self.chain.genesis.clone().into();
        let genesis_hash = genesis.hash_slow();
        let consensus = Arc::new(BeaconConsensus::new(self.chain.consensus.clone()));

        let network = config
            .network_config(noop_db, chain_id, genesis_hash, self.disable_discovery)
            .start_network()
            .await?;

        let fetch_client = Arc::new(network.fetch_client().await?);

        match self.command {
            Subcommands::Header { hash } => {
                let downloader = headers::linear::LinearDownloadBuilder::default()
                    .batch_size(config.stages.headers.downloader_batch_size)
                    .retries(config.stages.headers.downloader_retries)
                    .build(consensus.clone(), fetch_client.clone());

                // NOTE: head doesn't matter since we poll the stream only once
                let head = SealedHeader::default();
                match downloader.stream(head, hash).try_next().await {
                    Ok(Some(header)) => {
                        println!("Succesfully downloaded header: {header:?}");
                    }
                    Ok(None) => {
                        println!("No header response.")
                    }
                    Err(error) => {
                        println!("Encountered error: {error:?}");
                    }
                };
            }
        }

        Ok(())
    }
}

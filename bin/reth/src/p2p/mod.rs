//! P2P Debugging tool
use clap::{Parser, Subcommand};
use reth_db::mdbx::{Env, EnvKind, WriteMap};
use reth_interfaces::p2p::{
    bodies::client::BodiesClient,
    headers::client::{HeadersClient, HeadersRequest},
};
use reth_primitives::{BlockHashOrNumber, Header, H256};
use std::{ops::Range, sync::Arc};

use crate::{
    config::Config,
    dirs::ConfigPath,
    util::{
        chainspec::{chain_spec_value_parser, ChainSpecification},
        hash_or_num_value_parser,
    },
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
        /// The header number or hash
        #[arg(value_parser = hash_or_num_value_parser)]
        id: BlockHashOrNumber,
    },
    /// Download block body
    Body {
        /// The block hash to download
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

        let network = config
            .network_config(noop_db, chain_id, genesis_hash, self.disable_discovery)
            .start_network()
            .await?;

        let fetch_client = network.fetch_client().await?;

        match self.command {
            Subcommands::Header { id } => {
                let request = HeadersRequest {
                    direction: reth_primitives::HeadersDirection::Rising,
                    limit: 1,
                    start: id,
                };

                // TODO: check if the response is correct
                match fetch_client.get_headers(request).await {
                    Ok(result) => println!("Successfully downloaded header: {result:?}"),
                    Err(error) => println!("Encountered error: {error:?}"),
                };
            }
            Subcommands::Body { hash } => {
                // TODO: check if the response is correct
                match fetch_client.get_block_bodies(vec![hash]).await {
                    Ok(result) => println!("Successfully downloaded body: {result:?}"),
                    Err(error) => println!("Encountered error: {error:?}"),
                }
            }
        }

        Ok(())
    }
}

//! P2P Debugging tool
use crate::dirs::{ConfigPath, PlatformPath};
use backon::{ConstantBackoff, Retryable};
use clap::{Parser, Subcommand};
use reth_db::mdbx::{Env, EnvKind, WriteMap};
use reth_discv4::NatResolver;
use reth_interfaces::p2p::{
    bodies::client::BodiesClient,
    headers::client::{HeadersClient, HeadersRequest},
};
use reth_network::FetchClient;
use reth_primitives::{BlockHashOrNumber, ChainSpec, NodeRecord, SealedHeader};
use reth_staged_sync::{
    utils::{chainspec::chain_spec_value_parser, hash_or_num_value_parser},
    Config,
};
use std::sync::Arc;

/// `reth p2p` command
#[derive(Debug, Parser)]
pub struct Command {
    /// The path to the configuration file to use.
    #[arg(long, value_name = "FILE", verbatim_doc_comment, default_value_t)]
    config: PlatformPath<ConfigPath>,

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
    chain: ChainSpec,

    /// Disable the discovery service.
    #[arg(short, long)]
    disable_discovery: bool,

    /// Target trusted peer
    #[arg(long)]
    trusted_peer: Option<NodeRecord>,

    /// Connect only to trusted peers
    #[arg(long)]
    trusted_only: bool,

    /// The number of retries per request
    #[arg(long, default_value = "5")]
    retries: usize,

    #[clap(subcommand)]
    command: Subcommands,

    #[arg(long, default_value = "any")]
    nat: NatResolver,
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
        /// The block number or hash
        #[arg(value_parser = hash_or_num_value_parser)]
        id: BlockHashOrNumber,
    },
}
impl Command {
    /// Execute `p2p` command
    pub async fn execute(&self) -> eyre::Result<()> {
        let tempdir = tempfile::TempDir::new()?;
        let noop_db = Arc::new(Env::<WriteMap>::open(&tempdir.into_path(), EnvKind::RW)?);

        let mut config: Config = confy::load_path(&self.config).unwrap_or_default();

        if let Some(peer) = self.trusted_peer {
            config.peers.trusted_nodes.insert(peer);
        }

        if config.peers.trusted_nodes.is_empty() && self.trusted_only {
            eyre::bail!("No trusted nodes. Set trusted peer with `--trusted-peer <enode record>` or set `--trusted-only` to `false`")
        }

        config.peers.connect_trusted_nodes_only = self.trusted_only;

        let network = config
            .network_config(
                noop_db,
                self.chain.clone(),
                self.disable_discovery,
                None,
                self.nat,
                None,
            )
            .start_network()
            .await?;

        let fetch_client = network.fetch_client().await?;
        let retries = self.retries.max(1);
        let backoff = ConstantBackoff::default().with_max_times(retries);

        match self.command {
            Subcommands::Header { id } => {
                let header = (move || self.get_single_header(fetch_client.clone(), id))
                    .retry(backoff)
                    .notify(|err, _| println!("Error requesting header: {err}. Retrying..."))
                    .await?;
                println!("Successfully downloaded header: {header:?}");
            }
            Subcommands::Body { id } => {
                let hash = match id {
                    BlockHashOrNumber::Hash(hash) => hash,
                    BlockHashOrNumber::Number(number) => {
                        println!("Block number provided. Downloading header first...");
                        let client = fetch_client.clone();
                        let header = (move || {
                            self.get_single_header(
                                client.clone(),
                                BlockHashOrNumber::Number(number),
                            )
                        })
                        .retry(backoff.clone())
                        .notify(|err, _| println!("Error requesting header: {err}. Retrying..."))
                        .await?;
                        header.hash()
                    }
                };
                let (_, result) = (move || {
                    let client = fetch_client.clone();
                    async move { client.get_block_bodies(vec![hash]).await }
                })
                .retry(backoff)
                .notify(|err, _| println!("Error requesting block: {err}. Retrying..."))
                .await?
                .split();
                if result.len() != 1 {
                    eyre::bail!(
                        "Invalid number of headers received. Expected: 1. Received: {}",
                        result.len()
                    )
                }
                let body = result.into_iter().next().unwrap();
                println!("Successfully downloaded body: {body:?}")
            }
        }

        Ok(())
    }

    /// Get a single header from network
    pub async fn get_single_header(
        &self,
        client: FetchClient,
        id: BlockHashOrNumber,
    ) -> eyre::Result<SealedHeader> {
        let request = HeadersRequest {
            direction: reth_primitives::HeadersDirection::Rising,
            limit: 1,
            start: id,
        };

        let (_, response) = client.get_headers(request).await?.split();

        if response.len() != 1 {
            eyre::bail!(
                "Invalid number of headers received. Expected: 1. Received: {}",
                response.len()
            )
        }

        let header = response.into_iter().next().unwrap().seal();

        let valid = match id {
            BlockHashOrNumber::Hash(hash) => header.hash() == hash,
            BlockHashOrNumber::Number(number) => header.number == number,
        };

        if !valid {
            eyre::bail!(
                "Received invalid header. Received: {:?}. Expected: {:?}",
                header.num_hash(),
                id
            );
        }

        Ok(header)
    }
}

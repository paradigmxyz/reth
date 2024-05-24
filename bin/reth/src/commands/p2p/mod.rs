//! P2P Debugging tool

use crate::{
    args::{
        get_secret_key,
        utils::{chain_help, chain_spec_value_parser, hash_or_num_value_parser, SUPPORTED_CHAINS},
        DatabaseArgs, DiscoveryArgs, NetworkArgs,
    },
    dirs::{DataDirPath, MaybePlatformPath},
    utils::get_single_header,
};
use backon::{ConstantBuilder, Retryable};
use clap::{Parser, Subcommand};
use discv5::ListenConfig;
use reth_config::Config;
use reth_db::create_db;
use reth_interfaces::p2p::bodies::client::BodiesClient;
use reth_network::NetworkConfigBuilder;
use reth_primitives::{BlockHashOrNumber, ChainSpec};
use reth_provider::ProviderFactory;
use std::{
    net::{IpAddr, SocketAddrV4, SocketAddrV6},
    path::PathBuf,
    sync::Arc,
};

/// `reth p2p` command
#[derive(Debug, Parser)]
pub struct Command {
    /// The path to the configuration file to use.
    #[arg(long, value_name = "FILE", verbatim_doc_comment)]
    config: Option<PathBuf>,

    /// The chain this node is running.
    ///
    /// Possible values are either a built-in chain or the path to a chain specification file.
    #[arg(
        long,
        value_name = "CHAIN_OR_PATH",
        long_help = chain_help(),
        default_value = SUPPORTED_CHAINS[0],
        value_parser = chain_spec_value_parser
    )]
    chain: Arc<ChainSpec>,

    /// The path to the data dir for all reth files and subdirectories.
    ///
    /// Defaults to the OS-specific data directory:
    ///
    /// - Linux: `$XDG_DATA_HOME/reth/` or `$HOME/.local/share/reth/`
    /// - Windows: `{FOLDERID_RoamingAppData}/reth/`
    /// - macOS: `$HOME/Library/Application Support/reth/`
    #[arg(long, value_name = "DATA_DIR", verbatim_doc_comment, default_value_t)]
    datadir: MaybePlatformPath<DataDirPath>,

    /// Disable the discovery service.
    #[command(flatten)]
    pub network: NetworkArgs,

    /// The number of retries per request
    #[arg(long, default_value = "5")]
    retries: usize,

    #[command(flatten)]
    db: DatabaseArgs,

    #[command(subcommand)]
    command: Subcommands,
}

/// `reth p2p` subcommands
#[derive(Subcommand, Debug)]
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
        let noop_db = Arc::new(create_db(tempdir.into_path(), self.db.database_args())?);

        // add network name to data dir
        let data_dir = self.datadir.unwrap_or_chain_default(self.chain.chain);
        let config_path = self.config.clone().unwrap_or_else(|| data_dir.config());

        let mut config: Config = confy::load_path(&config_path).unwrap_or_default();

        for &peer in &self.network.trusted_peers {
            config.peers.trusted_nodes.insert(peer);
        }

        if config.peers.trusted_nodes.is_empty() && self.network.trusted_only {
            eyre::bail!("No trusted nodes. Set trusted peer with `--trusted-peer <enode record>` or set `--trusted-only` to `false`")
        }

        config.peers.trusted_nodes_only = self.network.trusted_only;

        let default_secret_key_path = data_dir.p2p_secret();
        let secret_key_path =
            self.network.p2p_secret_key.clone().unwrap_or(default_secret_key_path);
        let p2p_secret_key = get_secret_key(&secret_key_path)?;
        let rlpx_socket = (self.network.addr, self.network.port).into();
        let boot_nodes = self.chain.bootnodes().unwrap_or_default();

        let mut network_config_builder = NetworkConfigBuilder::new(p2p_secret_key)
            .peer_config(config.peers_config_with_basic_nodes_from_file(None))
            .external_ip_resolver(self.network.nat)
            .chain_spec(self.chain.clone())
            .disable_discv4_discovery_if(self.chain.chain.is_optimism())
            .boot_nodes(boot_nodes.clone());

        network_config_builder = self
            .network
            .discovery
            .apply_to_builder(network_config_builder, rlpx_socket)
            .map_discv5_config_builder(|builder| {
                let DiscoveryArgs {
                    discv5_addr,
                    discv5_addr_ipv6,
                    discv5_port,
                    discv5_port_ipv6,
                    discv5_lookup_interval,
                    discv5_bootstrap_lookup_interval,
                    discv5_bootstrap_lookup_countdown,
                    ..
                } = self.network.discovery;

                // Use rlpx address if none given
                let discv5_addr_ipv4 = discv5_addr.or(match self.network.addr {
                    IpAddr::V4(ip) => Some(ip),
                    IpAddr::V6(_) => None,
                });
                let discv5_addr_ipv6 = discv5_addr_ipv6.or(match self.network.addr {
                    IpAddr::V4(_) => None,
                    IpAddr::V6(ip) => Some(ip),
                });

                builder
                    .discv5_config(
                        discv5::ConfigBuilder::new(ListenConfig::from_two_sockets(
                            discv5_addr_ipv4.map(|addr| SocketAddrV4::new(addr, discv5_port)),
                            discv5_addr_ipv6
                                .map(|addr| SocketAddrV6::new(addr, discv5_port_ipv6, 0, 0)),
                        ))
                        .build(),
                    )
                    .add_unsigned_boot_nodes(boot_nodes.into_iter())
                    .lookup_interval(discv5_lookup_interval)
                    .bootstrap_lookup_interval(discv5_bootstrap_lookup_interval)
                    .bootstrap_lookup_countdown(discv5_bootstrap_lookup_countdown)
            });

        let network_config = network_config_builder.build(Arc::new(ProviderFactory::new(
            noop_db,
            self.chain.clone(),
            data_dir.static_files(),
        )?));

        let network = network_config.start_network().await?;
        let fetch_client = network.fetch_client().await?;
        let retries = self.retries.max(1);
        let backoff = ConstantBuilder::default().with_max_times(retries);

        match self.command {
            Subcommands::Header { id } => {
                let header = (move || get_single_header(fetch_client.clone(), id))
                    .retry(&backoff)
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
                            get_single_header(client.clone(), BlockHashOrNumber::Number(number))
                        })
                        .retry(&backoff)
                        .notify(|err, _| println!("Error requesting header: {err}. Retrying..."))
                        .await?;
                        header.hash()
                    }
                };
                let (_, result) = (move || {
                    let client = fetch_client.clone();
                    client.get_block_bodies(vec![hash])
                })
                .retry(&backoff)
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
}

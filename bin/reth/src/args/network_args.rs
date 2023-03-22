//! clap [Args](clap::Args) for network related arguments.

use crate::dirs::{KnownPeersPath, PlatformPath};
use clap::Args;
use reth_net_nat::NatResolver;
use reth_network::NetworkConfigBuilder;
use reth_primitives::{mainnet_nodes, ChainSpec, NodeRecord};
use reth_staged_sync::Config;
use std::{path::PathBuf, sync::Arc};

/// Parameters for configuring the network more granularity via CLI
#[derive(Debug, Args)]
#[command(next_help_heading = "Networking")]
pub struct NetworkArgs {
    /// Disable the discovery service.
    #[command(flatten)]
    pub discovery: DiscoveryArgs,

    /// Target trusted peer enodes
    /// --trusted-peers enode://abcd@192.168.0.1:30303
    #[arg(long)]
    pub trusted_peers: Vec<NodeRecord>,

    /// Connect only to trusted peers
    #[arg(long)]
    pub trusted_only: bool,

    /// Bootnodes to connect to initially.
    ///
    /// Will fall back to a network-specific default if not specified.
    #[arg(long, value_delimiter = ',')]
    pub bootnodes: Option<Vec<NodeRecord>>,

    /// The path to the known peers file. Connected peers are
    /// dumped to this file on node shutdown, and read on startup.
    /// Cannot be used with --no-persist-peers
    #[arg(long, value_name = "FILE", verbatim_doc_comment, default_value_t)]
    pub peers_file: PlatformPath<KnownPeersPath>,

    /// Do not persist peers. Cannot be used with --peers-file
    #[arg(long, verbatim_doc_comment, conflicts_with = "peers_file")]
    pub no_persist_peers: bool,

    /// NAT resolution method.
    #[arg(long, default_value = "any")]
    pub nat: NatResolver,

    /// Network listening port. default: 30303
    #[arg(long = "port", value_name = "PORT")]
    pub port: Option<u16>,
}

impl NetworkArgs {
    /// Build a [`NetworkConfigBuilder`] from a [`Config`] and a [`ChainSpec`], in addition to the
    /// values in this option struct.
    pub fn network_config(
        &self,
        config: &Config,
        chain_spec: Arc<ChainSpec>,
    ) -> NetworkConfigBuilder {
        let chain_bootnodes = chain_spec.chain.bootnodes().unwrap_or_else(mainnet_nodes);

        let network_config_builder = config
            .network_config(self.nat, self.persistent_peers_file())
            .boot_nodes(self.bootnodes.clone().unwrap_or(chain_bootnodes))
            .chain_spec(chain_spec);

        self.discovery.apply_to_builder(network_config_builder)
    }
}

// === impl NetworkArgs ===

impl NetworkArgs {
    /// If `no_persist_peers` is true then this returns the path to the persistent peers file
    pub fn persistent_peers_file(&self) -> Option<PathBuf> {
        if self.no_persist_peers {
            return None
        }
        Some(self.peers_file.clone().into())
    }
}

/// Arguments to setup discovery
#[derive(Debug, Args)]
pub struct DiscoveryArgs {
    /// Disable the discovery service.
    #[arg(short, long)]
    pub disable_discovery: bool,

    /// Disable the DNS discovery.
    #[arg(long, conflicts_with = "disable_discovery")]
    pub disable_dns_discovery: bool,

    /// Disable Discv4 discovery.
    #[arg(long, conflicts_with = "disable_discovery")]
    pub disable_discv4_discovery: bool,

    /// The UDP port to use for P2P discovery/networking. default: 30303
    #[arg(long = "discovery.port", name = "discovery.port", value_name = "DISCOVERY_PORT")]
    pub port: Option<u16>,
}

impl DiscoveryArgs {
    /// Apply the discovery settings to the given [NetworkConfigBuilder]
    pub fn apply_to_builder(
        &self,
        mut network_config_builder: NetworkConfigBuilder,
    ) -> NetworkConfigBuilder {
        if self.disable_discovery || self.disable_dns_discovery {
            network_config_builder = network_config_builder.disable_dns_discovery();
        }

        if self.disable_discovery || self.disable_discv4_discovery {
            network_config_builder = network_config_builder.disable_discv4_discovery();
        }
        network_config_builder
    }
}

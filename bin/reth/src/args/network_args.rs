//! clap [Args](clap::Args) for network related arguments.

use crate::dirs::{KnownPeersPath, PlatformPath};
use clap::Args;
use reth_discv4::bootnodes::mainnet_nodes;
use reth_net_nat::NatResolver;
use reth_network::NetworkConfigBuilder;
use reth_primitives::{ChainSpec, NodeRecord};
use reth_staged_sync::Config;
use std::path::PathBuf;

/// Parameters for configuring the network more granularity via CLI
#[derive(Debug, Args)]
#[command(next_help_heading = "Networking")]
pub struct NetworkArgs {
    /// Disable the discovery service.
    #[arg(short, long)]
    pub disable_discovery: bool,

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
}

impl NetworkArgs {
    /// Build a [`NetworkConfigBuilder`] from a [`Config`] and a [`ChainSpec`], in addition to the
    /// values in this option struct.
    pub fn network_config(&self, config: &Config, chain_spec: ChainSpec) -> NetworkConfigBuilder {
        let peers_file = (!self.no_persist_peers).then_some(&self.peers_file);
        config
            .network_config(self.nat, peers_file.map(|f| f.as_ref().to_path_buf()))
            .boot_nodes(self.bootnodes.clone().unwrap_or_else(mainnet_nodes))
            .chain_spec(chain_spec)
            .set_discovery(self.disable_discovery)
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

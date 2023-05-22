//! clap [Args](clap::Args) for network related arguments.

use crate::version::P2P_CLIENT_VERSION;
use clap::Args;
use reth_net_nat::NatResolver;
use reth_network::{HelloMessage, NetworkConfigBuilder};
use reth_primitives::{mainnet_nodes, ChainSpec, NodeRecord};
use reth_staged_sync::Config;
use secp256k1::SecretKey;
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

    /// The path to the known peers file. Connected peers are dumped to this file on nodes
    /// shutdown, and read on startup. Cannot be used with `--no-persist-peers`.
    #[arg(long, value_name = "FILE", verbatim_doc_comment, conflicts_with = "no_persist_peers")]
    pub peers_file: Option<PathBuf>,

    /// Custom node identity
    #[arg(long, value_name = "IDENTITY", default_value = P2P_CLIENT_VERSION)]
    pub identity: String,

    /// Secret key to use for this node.
    ///
    /// This will also deterministically set the peer ID. If not specified, it will be set in the
    /// data dir for the chain being used.
    #[arg(long, value_name = "PATH")]
    pub p2p_secret_key: Option<PathBuf>,

    /// Do not persist peers.
    #[arg(long, verbatim_doc_comment)]
    pub no_persist_peers: bool,

    #[allow(rustdoc::invalid_html_tags)]
    /// NAT resolution method (any|none|upnp|publicip|extip:<IP>)
    #[arg(long, default_value = "any")]
    pub nat: NatResolver,

    /// Network listening port. default: 30303
    #[arg(long = "port", value_name = "PORT")]
    pub port: Option<u16>,
}

impl NetworkArgs {
    /// Build a [`NetworkConfigBuilder`] from a [`Config`] and a [`ChainSpec`], in addition to the
    /// values in this option struct.
    ///
    /// The `default_peers_file` will be used as the default location to store the persistent peers
    /// file if `no_persist_peers` is false, and there is no provided `peers_file`.
    pub fn network_config(
        &self,
        config: &Config,
        chain_spec: Arc<ChainSpec>,
        secret_key: SecretKey,
        default_peers_file: PathBuf,
    ) -> NetworkConfigBuilder {
        let chain_bootnodes = chain_spec.chain.bootnodes().unwrap_or_else(mainnet_nodes);
        let peers_file = self.peers_file.clone().unwrap_or(default_peers_file);

        // Configure basic network stack.
        let mut network_config_builder = config
            .network_config(self.nat, self.persistent_peers_file(peers_file), secret_key)
            .boot_nodes(self.bootnodes.clone().unwrap_or(chain_bootnodes))
            .chain_spec(chain_spec);

        // Configure node identity
        let peer_id = network_config_builder.get_peer_id();
        network_config_builder = network_config_builder
            .hello_message(HelloMessage::builder(peer_id).client_version(&self.identity).build());

        self.discovery.apply_to_builder(network_config_builder)
    }
}

// === impl NetworkArgs ===

impl NetworkArgs {
    /// If `no_persist_peers` is true then this returns the path to the persistent peers file path.
    pub fn persistent_peers_file(&self, peers_file: PathBuf) -> Option<PathBuf> {
        if self.no_persist_peers {
            return None
        }

        Some(peers_file)
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

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    /// A helper type to parse Args more easily
    #[derive(Parser)]
    struct CommandParser<T: Args> {
        #[clap(flatten)]
        args: T,
    }

    #[test]
    fn parse_nat_args() {
        let args = CommandParser::<NetworkArgs>::parse_from(["reth", "--nat", "none"]).args;
        assert_eq!(args.nat, NatResolver::None);

        let args =
            CommandParser::<NetworkArgs>::parse_from(["reth", "--nat", "extip:0.0.0.0"]).args;
        assert_eq!(args.nat, NatResolver::ExternalIp("0.0.0.0".parse().unwrap()));
    }
}

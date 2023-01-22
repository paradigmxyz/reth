#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]
//! Rust Ethereum (reth) binary executable.

pub mod cli;
pub mod db;
pub mod dirs;
pub mod node;
pub mod p2p;
pub mod prometheus_exporter;
pub mod stage;
pub mod test_eth_chain;
use reth_db::database::Database;
use reth_network::NetworkConfig;
use reth_provider::ProviderImpl;
pub use reth_staged_sync::utils;
use reth_staged_sync::Config;

use clap::Args;
use reth_primitives::{ChainSpec, NodeRecord};

use reth_net_nat::NatResolver;
use std::sync::Arc;

/// Parameters for configuring the network more granularly via CLI
#[derive(Debug, Args)]
#[command(next_help_heading = "Networking")]
struct NetworkOpts {
    /// Disable the discovery service.
    #[arg(short, long)]
    disable_discovery: bool,

    /// Target trusted peer enodes
    /// --trusted-peers enode://abcd@192.168.0.1:30303
    #[arg(long)]
    trusted_peers: Vec<NodeRecord>,

    /// Connect only to trusted peers
    #[arg(long)]
    trusted_only: bool,

    /// Bootnodes to connect to initially.
    ///
    /// Will fall back to a network-specific default if not specified.
    #[arg(long, value_delimiter = ',')]
    bootnodes: Option<Vec<NodeRecord>>,

    #[arg(long, default_value = "any")]
    nat: NatResolver,
}

impl NetworkOpts {
    pub(crate) fn config<DB: Database>(
        &self,
        config: &mut Config,
        chain: ChainSpec,
        db: Arc<DB>,
    ) -> NetworkConfig<ProviderImpl<DB>> {
        config.peers.connect_trusted_nodes_only = self.trusted_only;

        if !self.trusted_peers.is_empty() {
            self.trusted_peers.iter().for_each(|peer| {
                config.peers.trusted_nodes.insert(*peer);
            });
        }

        config.network_config(db, chain, self.disable_discovery, self.bootnodes.clone(), self.nat)
    }
}

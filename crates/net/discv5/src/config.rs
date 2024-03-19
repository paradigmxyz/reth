//! Wrapper around [`discv5::Config`].

use std::{collections::HashSet, net::SocketAddr};

use discv5::ListenConfig;
#[cfg(feature = "optimism")]
use reth_primitives::BASE_MAINNET;
#[cfg(not(feature = "optimism"))]
use reth_primitives::MAINNET;
use reth_primitives::{ForkId, Hardfork};

/// Builds a [`DiscV5Config`].
#[derive(Debug, Default)]
pub struct DiscV5ConfigBuilder {
    /// Config used by [`discv5::Discv5`].
    discv5_config: Option<discv5::Config>,
    /// Nodes to boot from.
    bootstrap_nodes: HashSet<discv5::Enr>,
    /// [`ForkId`] to set in local node record.
    fork_id: Option<ForkId>,
}

impl DiscV5ConfigBuilder {
    /// Returns a new builder, with all fields set like given instance.
    pub fn new_from(discv5_config: DiscV5Config) -> Self {
        let DiscV5Config { discv5_config, bootstrap_nodes, fork_id } = discv5_config;

        Self { discv5_config: Some(discv5_config), bootstrap_nodes, fork_id: Some(fork_id) }
    }

    /// Set [`discv5::Config`], which contains the [`discv5::Discv5`] listen socket.
    pub fn discv5_config(mut self, discv5_config: discv5::Config) -> Self {
        self.discv5_config = Some(discv5_config);
        self
    }

    /// Adds a boot node. Returns `true` if set didn't already contain the node.
    pub fn boot_node(mut self, node: discv5::Enr) -> Self {
        self.bootstrap_nodes.insert(node);
        self
    }

    /// Adds multiple boot nodes.
    pub fn extend_boot_nodes(mut self, nodes: impl IntoIterator<Item = discv5::Enr>) -> Self {
        self.bootstrap_nodes.extend(nodes);
        self
    }

    /// Set [`ForkId`] to include in the local [`Enr`](discv5::enr::Enr).
    pub fn fork_id(mut self, fork_id: ForkId) -> Self {
        self.fork_id = Some(fork_id);
        self
    }

    /// Returns a new [`DiscV5Config`].
    pub fn build(self) -> DiscV5Config {
        let Self { discv5_config, bootstrap_nodes, fork_id } = self;

        let discv5_config = discv5_config
            .unwrap_or_else(|| discv5::ConfigBuilder::new(ListenConfig::default()).build());

        #[cfg(not(feature = "optimism"))]
        let fork_id =
            fork_id.unwrap_or_else(|| MAINNET.hardfork_fork_id(Hardfork::latest()).unwrap());
        #[cfg(feature = "optimism")]
        let fork_id =
            fork_id.unwrap_or_else(|| BASE_MAINNET.hardfork_fork_id(Hardfork::latest()).unwrap());

        DiscV5Config { discv5_config, bootstrap_nodes, fork_id }
    }
}

/// Config used to bootstrap [`discv5::Discv5`].
#[derive(Debug)]
pub struct DiscV5Config {
    /// Config used by [`discv5::Discv5`].
    discv5_config: discv5::Config,
    /// Nodes to boot from.
    bootstrap_nodes: HashSet<discv5::Enr>,
    /// [`ForkId`] to set in local node record.
    fork_id: ForkId,
}

impl DiscV5Config {
    /// Returns the socket contained in the [`discv5::Config`]. Returns the IPv6 socket, if both
    /// IPv4 and v6 are configured.
    pub fn socket(&self) -> SocketAddr {
        match self.discv5_config.listen_config {
            ListenConfig::Ipv4 { ip, port } => (ip, port).into(),
            ListenConfig::Ipv6 { ip, port } => (ip, port).into(),
            ListenConfig::DualStack { ipv6, ipv6_port, .. } => (ipv6, ipv6_port).into(),
        }
    }

    /// Destructs the config.
    pub fn destruct(self) -> (discv5::Config, HashSet<discv5::Enr>, ForkId) {
        let Self { discv5_config, bootstrap_nodes, fork_id } = self;

        (discv5_config, bootstrap_nodes, fork_id)
    }
}

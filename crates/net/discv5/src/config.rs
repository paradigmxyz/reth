//! Wrapper around [`discv5::Config`].

use std::{collections::HashSet, net::SocketAddr};

use discv5::ListenConfig;
use reth_discv4::DEFAULT_DISCOVERY_PORT;
use reth_primitives::{Bytes, ForkId, Hardfork, MAINNET};

/// Builds a [`DiscV5Config`].
#[derive(Debug, Default)]
pub struct DiscV5ConfigBuilder {
    /// Config used by [`discv5::Discv5`]. Contains the discovery listen socket.
    discv5_config: Option<discv5::Config>,
    /// Nodes to boot from.
    bootstrap_nodes: HashSet<discv5::Enr>,
    /// [`ForkId`] to set in local node record.
    fork_id: Option<ForkId>,
    /// Mempool TCP port to advertise.
    tcp_port: Option<u16>,
    /// Additional kv-pairs to include in local node record.
    additional_local_enr_data: Vec<(&'static str, Bytes)>,
    /// Allow no TCP port advertised by discovered nodes. Disallowed by default.
    allow_no_tcp_discovered_nodes: bool,
}

impl DiscV5ConfigBuilder {
    /// Returns a new builder, with all fields set like given instance.
    pub fn new_from(discv5_config: DiscV5Config) -> Self {
        let DiscV5Config {
            discv5_config,
            bootstrap_nodes,
            fork_id,
            tcp_port,
            additional_local_enr_data,
            allow_no_tcp_discovered_nodes,
        } = discv5_config;

        Self {
            discv5_config: Some(discv5_config),
            bootstrap_nodes,
            fork_id: Some(fork_id),
            tcp_port: Some(tcp_port),
            additional_local_enr_data,
            allow_no_tcp_discovered_nodes,
        }
    }

    /// Set [`discv5::Config`], which contains the [`discv5::Discv5`] listen socket.
    pub fn discv5_config(mut self, discv5_config: discv5::Config) -> Self {
        self.discv5_config = Some(discv5_config);
        self
    }

    /// Adds a boot node. Returns `true` if set didn't already contain the node.
    pub fn add_boot_node(mut self, node: discv5::Enr) -> Self {
        self.bootstrap_nodes.insert(node);
        self
    }

    /// Adds multiple boot nodes.
    pub fn add_boot_nodes(mut self, nodes: impl IntoIterator<Item = discv5::Enr>) -> Self {
        self.bootstrap_nodes.extend(nodes);
        self
    }

    /// Parses a comma-separated list of serialized [`Enr`]s and adds any successfully deserialized
    /// records to boot nodes.
    pub fn parse_and_add_boot_nodes(mut self, nodes: &str) -> Self {
        let bootstrap_nodes = &mut self.bootstrap_nodes;
        for res in nodes.split(&[',']).map(|record| record.trim().parse::<discv5::Enr>()) {
            if let Ok(node) = res {
                bootstrap_nodes.insert(node);
            }
        }
        self
    }

    /// Set [`ForkId`] to include in the local [`Enr`](discv5::enr::Enr).
    pub fn fork_id(mut self, fork_id: ForkId) -> Self {
        self.fork_id = Some(fork_id);
        self
    }

    /// Sets the tcp port to advertise in the local [`Enr`](discv5::enr::Enr).
    pub fn tcp_port(mut self, port: u16) -> Self {
        self.tcp_port = Some(port);
        self
    }

    /// Allows discovered nodes without tcp port set in their ENR to be passed from
    /// [`discv5::Discv5`] up to the app.
    pub fn allow_no_tcp_discovered_nodes(mut self) -> Self {
        self.allow_no_tcp_discovered_nodes = true;
        self
    }

    /// Returns a new [`DiscV5Config`].
    pub fn build(self) -> DiscV5Config {
        let Self {
            discv5_config,
            bootstrap_nodes,
            fork_id,
            tcp_port,
            additional_local_enr_data,
            allow_no_tcp_discovered_nodes,
        } = self;

        let discv5_config = discv5_config
            .unwrap_or_else(|| discv5::ConfigBuilder::new(ListenConfig::default()).build());

        let fork_id =
            fork_id.unwrap_or_else(|| MAINNET.hardfork_fork_id(Hardfork::latest()).unwrap());

        let tcp_port = tcp_port.unwrap_or(DEFAULT_DISCOVERY_PORT);

        DiscV5Config {
            discv5_config,
            bootstrap_nodes,
            fork_id,
            tcp_port,
            additional_local_enr_data,
            allow_no_tcp_discovered_nodes,
        }
    }
}

/// Config used to bootstrap [`discv5::Discv5`].
#[derive(Debug)]
pub struct DiscV5Config {
    /// Config used by [`discv5::Discv5`]. Contains the [`ListenConfig`], with the discovery listen
    /// socket.
    discv5_config: discv5::Config,
    /// Nodes to boot from.
    bootstrap_nodes: HashSet<discv5::Enr>,
    /// [`ForkId`] to set in local node record.
    fork_id: ForkId,
    /// Mempool TCP port to advertise.
    tcp_port: u16,
    /// Additional kv-pairs to include in local node record.
    additional_local_enr_data: Vec<(&'static str, Bytes)>,
    /// Allow no TCP port advertised by discovered nodes. Disallowed by default.
    allow_no_tcp_discovered_nodes: bool,
}

impl DiscV5Config {
    /// Returns a new [`DiscV5ConfigBuilder`].
    pub fn builder() -> DiscV5ConfigBuilder {
        DiscV5ConfigBuilder::default()
    }

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
    pub fn destruct(
        self,
    ) -> (discv5::Config, HashSet<discv5::Enr>, ForkId, u16, Vec<(&'static str, Bytes)>, bool) {
        let Self {
            discv5_config,
            bootstrap_nodes,
            fork_id,
            tcp_port,
            additional_local_enr_data,
            allow_no_tcp_discovered_nodes,
        } = self;

        (
            discv5_config,
            bootstrap_nodes,
            fork_id,
            tcp_port,
            additional_local_enr_data,
            allow_no_tcp_discovered_nodes,
        )
    }
}

#[cfg(test)]
mod test {
    use std::net::SocketAddrV4;

    use reth_primitives::hex;

    use crate::DiscV5Config;

    #[test]
    fn parse_boot_nodes() {
        const OP_SEPOLIA_NODE_P2P_BOOTNODES: &'static str ="enr:-J64QBwRIWAco7lv6jImSOjPU_W266lHXzpAS5YOh7WmgTyBZkgLgOwo_mxKJq3wz2XRbsoBItbv1dCyjIoNq67mFguGAYrTxM42gmlkgnY0gmlwhBLSsHKHb3BzdGFja4S0lAUAiXNlY3AyNTZrMaEDmoWSi8hcsRpQf2eJsNUx-sqv6fH4btmo2HsAzZFAKnKDdGNwgiQGg3VkcIIkBg,enr:-J64QFa3qMsONLGphfjEkeYyF6Jkil_jCuJmm7_a42ckZeUQGLVzrzstZNb1dgBp1GGx9bzImq5VxJLP-BaptZThGiWGAYrTytOvgmlkgnY0gmlwhGsV-zeHb3BzdGFja4S0lAUAiXNlY3AyNTZrMaEDahfSECTIS_cXyZ8IyNf4leANlZnrsMEWTkEYxf4GMCmDdGNwgiQGg3VkcIIkBg";

        let config =
            DiscV5Config::builder().parse_and_add_boot_nodes(OP_SEPOLIA_NODE_P2P_BOOTNODES).build();

        let socket_1 = "18.210.176.114:9222".parse::<SocketAddrV4>().unwrap();
        let socket_2 = "107.21.251.55:9222".parse::<SocketAddrV4>().unwrap();

        for node in config.bootstrap_nodes {
            assert!(
                socket_1 == node.udp4_socket().unwrap() && socket_1 == node.tcp4_socket().unwrap() ||
                    socket_2 == node.udp4_socket().unwrap() &&
                        socket_2 == node.tcp4_socket().unwrap()
            );
            assert_eq!("84b4940500", hex::encode(node.get_raw_rlp("opstack").unwrap()));
        }
    }
}

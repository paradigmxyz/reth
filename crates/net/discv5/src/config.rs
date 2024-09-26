//! Wrapper around [`discv5::Config`].

use std::{
    collections::HashSet,
    fmt::Debug,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6},
};

use alloy_primitives::Bytes;
use derive_more::Display;
use discv5::{
    multiaddr::{Multiaddr, Protocol},
    ListenConfig,
};
use reth_ethereum_forks::{EnrForkIdEntry, ForkId};
use reth_network_peers::NodeRecord;
use tracing::warn;

use crate::{enr::discv4_id_to_multiaddr_id, filter::MustNotIncludeKeys, NetworkStackId};

/// The default address for discv5 via UDP is IPv4.
///
/// Default is 0.0.0.0, all interfaces. See [`discv5::ListenConfig`] default.
pub const DEFAULT_DISCOVERY_V5_ADDR: Ipv4Addr = Ipv4Addr::UNSPECIFIED;

/// The default IPv6 address for discv5 via UDP.
///
/// Default is ::, all interfaces.
pub const DEFAULT_DISCOVERY_V5_ADDR_IPV6: Ipv6Addr = Ipv6Addr::UNSPECIFIED;

/// The default port for discv5 via UDP.
///
/// Default is port 9200.
pub const DEFAULT_DISCOVERY_V5_PORT: u16 = 9200;

/// The default [`discv5::ListenConfig`].
///
/// This is different from the upstream default.
pub const DEFAULT_DISCOVERY_V5_LISTEN_CONFIG: ListenConfig =
    ListenConfig::Ipv4 { ip: DEFAULT_DISCOVERY_V5_ADDR, port: DEFAULT_DISCOVERY_V5_PORT };

/// Default interval in seconds at which to run a lookup up query.
///
/// Default is 60 seconds.
pub const DEFAULT_SECONDS_LOOKUP_INTERVAL: u64 = 60;

/// Default number of times to do pulse lookup queries, at bootstrap (pulse intervals, defaulting
/// to 5 seconds).
///
/// Default is 100 counts.
pub const DEFAULT_COUNT_BOOTSTRAP_LOOKUPS: u64 = 100;

/// Default duration of the pulse lookup interval at bootstrap.
///
/// Default is 5 seconds.
pub const DEFAULT_SECONDS_BOOTSTRAP_LOOKUP_INTERVAL: u64 = 5;

/// Builds a [`Config`].
#[derive(Debug)]
pub struct ConfigBuilder {
    /// Config used by [`discv5::Discv5`]. Contains the discovery listen socket.
    discv5_config: Option<discv5::Config>,
    /// Nodes to boot from.
    bootstrap_nodes: HashSet<BootNode>,
    /// Fork kv-pair to set in local node record. Identifies which network/chain/fork the node
    /// belongs, e.g. `(b"opstack", ChainId)` or `(b"eth", ForkId)`.
    ///
    /// Defaults to L1 mainnet if not set.
    fork: Option<(&'static [u8], ForkId)>,
    /// `RLPx` TCP socket to advertise.
    ///
    /// NOTE: IP address of `RLPx` socket overwrites IP address of same IP version in
    /// [`discv5::ListenConfig`].
    tcp_socket: SocketAddr,
    /// List of `(key, rlp-encoded-value)` tuples that should be advertised in local node record
    /// (in addition to tcp port, udp port and fork).
    other_enr_kv_pairs: Vec<(&'static [u8], Bytes)>,
    /// Interval in seconds at which to run a lookup up query to populate kbuckets.
    lookup_interval: Option<u64>,
    /// Interval in seconds at which to run pulse lookup queries at bootstrap to boost kbucket
    /// population.
    bootstrap_lookup_interval: Option<u64>,
    /// Number of times to run boost lookup queries at start up.
    bootstrap_lookup_countdown: Option<u64>,
    /// Custom filter rules to apply to a discovered peer in order to determine if it should be
    /// passed up to rlpx or dropped.
    discovered_peer_filter: Option<MustNotIncludeKeys>,
}

impl ConfigBuilder {
    /// Returns a new builder, with all fields set like given instance.
    pub fn new_from(discv5_config: Config) -> Self {
        let Config {
            discv5_config,
            bootstrap_nodes,
            fork,
            tcp_socket,
            other_enr_kv_pairs,
            lookup_interval,
            bootstrap_lookup_interval,
            bootstrap_lookup_countdown,
            discovered_peer_filter,
        } = discv5_config;

        Self {
            discv5_config: Some(discv5_config),
            bootstrap_nodes,
            fork: fork.map(|(key, fork_id)| (key, fork_id.fork_id)),
            tcp_socket,
            other_enr_kv_pairs,
            lookup_interval: Some(lookup_interval),
            bootstrap_lookup_interval: Some(bootstrap_lookup_interval),
            bootstrap_lookup_countdown: Some(bootstrap_lookup_countdown),
            discovered_peer_filter: Some(discovered_peer_filter),
        }
    }

    /// Set [`discv5::Config`], which contains the [`discv5::Discv5`] listen socket.
    pub fn discv5_config(mut self, discv5_config: discv5::Config) -> Self {
        self.discv5_config = Some(discv5_config);
        self
    }

    /// Adds multiple boot nodes from a list of [`Enr`](discv5::Enr)s.
    pub fn add_signed_boot_nodes(mut self, nodes: impl IntoIterator<Item = discv5::Enr>) -> Self {
        self.bootstrap_nodes.extend(nodes.into_iter().map(BootNode::Enr));
        self
    }

    /// Parses a comma-separated list of serialized [`Enr`](discv5::Enr)s, signed node records, and
    /// adds any successfully deserialized records to boot nodes. Note: this type is serialized in
    /// CL format since [`discv5`] is originally a CL library.
    pub fn add_cl_serialized_signed_boot_nodes(mut self, enrs: &str) -> Self {
        let bootstrap_nodes = &mut self.bootstrap_nodes;
        for node in enrs.split(&[',']).flat_map(|record| record.trim().parse::<discv5::Enr>()) {
            bootstrap_nodes.insert(BootNode::Enr(node));
        }
        self
    }

    /// Adds boot nodes in the form a list of [`NodeRecord`]s, parsed enodes.
    pub fn add_unsigned_boot_nodes(mut self, enodes: impl IntoIterator<Item = NodeRecord>) -> Self {
        for node in enodes {
            if let Ok(node) = BootNode::from_unsigned(node) {
                self.bootstrap_nodes.insert(node);
            }
        }

        self
    }

    /// Adds a comma-separated list of enodes, serialized unsigned node records, to boot nodes.
    pub fn add_serialized_unsigned_boot_nodes(mut self, enodes: &[&str]) -> Self {
        for node in enodes {
            if let Ok(node) = node.parse() {
                if let Ok(node) = BootNode::from_unsigned(node) {
                    self.bootstrap_nodes.insert(node);
                }
            }
        }

        self
    }

    /// Set fork ID kv-pair to set in local [`Enr`](discv5::enr::Enr). This lets peers on discovery
    /// network know which chain this node belongs to.
    pub const fn fork(mut self, fork_key: &'static [u8], fork_id: ForkId) -> Self {
        self.fork = Some((fork_key, fork_id));
        self
    }

    /// Sets the tcp socket to advertise in the local [`Enr`](discv5::enr::Enr). The IP address of
    /// this socket will overwrite the discovery address of the same IP version, if one is
    /// configured.
    pub const fn tcp_socket(mut self, socket: SocketAddr) -> Self {
        self.tcp_socket = socket;
        self
    }

    /// Adds an additional kv-pair to include in the local [`Enr`](discv5::enr::Enr). Takes the key
    /// to use for the kv-pair and the rlp encoded value.
    pub fn add_enr_kv_pair(mut self, key: &'static [u8], value: Bytes) -> Self {
        self.other_enr_kv_pairs.push((key, value));
        self
    }

    /// Sets the interval at which to run lookup queries, in order to fill kbuckets. Lookup queries
    /// are done periodically at the given interval for the whole run of the program.
    pub const fn lookup_interval(mut self, seconds: u64) -> Self {
        self.lookup_interval = Some(seconds);
        self
    }

    /// Sets the interval at which to run boost lookup queries at start up. Queries will be started
    /// at this interval for the configured number of times after start up.
    pub const fn bootstrap_lookup_interval(mut self, seconds: u64) -> Self {
        self.bootstrap_lookup_interval = Some(seconds);
        self
    }

    /// Sets the number of times at which to run boost lookup queries to bootstrap the node.
    pub const fn bootstrap_lookup_countdown(mut self, counts: u64) -> Self {
        self.bootstrap_lookup_countdown = Some(counts);
        self
    }

    /// Adds keys to disallow when filtering a discovered peer, to determine whether or not it
    /// should be passed to rlpx. The discovered node record is scanned for any kv-pairs where the
    /// key matches the disallowed keys. If not explicitly set, b"eth2" key will be disallowed.
    pub fn must_not_include_keys(mut self, not_keys: &[&'static [u8]]) -> Self {
        let mut filter = self.discovered_peer_filter.unwrap_or_default();
        filter.add_disallowed_keys(not_keys);
        self.discovered_peer_filter = Some(filter);
        self
    }

    /// Returns a new [`Config`].
    pub fn build(self) -> Config {
        let Self {
            discv5_config,
            bootstrap_nodes,
            fork,
            tcp_socket,
            other_enr_kv_pairs,
            lookup_interval,
            bootstrap_lookup_interval,
            bootstrap_lookup_countdown,
            discovered_peer_filter,
        } = self;

        let mut discv5_config = discv5_config.unwrap_or_else(|| {
            discv5::ConfigBuilder::new(DEFAULT_DISCOVERY_V5_LISTEN_CONFIG).build()
        });

        discv5_config.listen_config =
            amend_listen_config_wrt_rlpx(&discv5_config.listen_config, tcp_socket.ip());

        let fork = fork.map(|(key, fork_id)| (key, fork_id.into()));

        let lookup_interval = lookup_interval.unwrap_or(DEFAULT_SECONDS_LOOKUP_INTERVAL);
        let bootstrap_lookup_interval =
            bootstrap_lookup_interval.unwrap_or(DEFAULT_SECONDS_BOOTSTRAP_LOOKUP_INTERVAL);
        let bootstrap_lookup_countdown =
            bootstrap_lookup_countdown.unwrap_or(DEFAULT_COUNT_BOOTSTRAP_LOOKUPS);

        let discovered_peer_filter = discovered_peer_filter
            .unwrap_or_else(|| MustNotIncludeKeys::new(&[NetworkStackId::ETH2]));

        Config {
            discv5_config,
            bootstrap_nodes,
            fork,
            tcp_socket,
            other_enr_kv_pairs,
            lookup_interval,
            bootstrap_lookup_interval,
            bootstrap_lookup_countdown,
            discovered_peer_filter,
        }
    }
}

/// Config used to bootstrap [`discv5::Discv5`].
#[derive(Clone, Debug)]
pub struct Config {
    /// Config used by [`discv5::Discv5`]. Contains the [`ListenConfig`], with the discovery listen
    /// socket.
    pub(super) discv5_config: discv5::Config,
    /// Nodes to boot from.
    pub(super) bootstrap_nodes: HashSet<BootNode>,
    /// Fork kv-pair to set in local node record. Identifies which network/chain/fork the node
    /// belongs, e.g. `(b"opstack", ChainId)` or `(b"eth", [ForkId])`.
    pub(super) fork: Option<(&'static [u8], EnrForkIdEntry)>,
    /// `RLPx` TCP socket to advertise.
    ///
    /// NOTE: IP address of `RLPx` socket overwrites IP address of same IP version in
    /// [`discv5::ListenConfig`].
    pub(super) tcp_socket: SocketAddr,
    /// Additional kv-pairs (besides tcp port, udp port and fork) that should be advertised to
    /// peers by including in local node record.
    pub(super) other_enr_kv_pairs: Vec<(&'static [u8], Bytes)>,
    /// Interval in seconds at which to run a lookup up query with to populate kbuckets.
    pub(super) lookup_interval: u64,
    /// Interval in seconds at which to run pulse lookup queries at bootstrap to boost kbucket
    /// population.
    pub(super) bootstrap_lookup_interval: u64,
    /// Number of times to run boost lookup queries at start up.
    pub(super) bootstrap_lookup_countdown: u64,
    /// Custom filter rules to apply to a discovered peer in order to determine if it should be
    /// passed up to rlpx or dropped.
    pub(super) discovered_peer_filter: MustNotIncludeKeys,
}

impl Config {
    /// Returns a new [`ConfigBuilder`], with the `RLPx` TCP port and IP version configured w.r.t.
    /// the given socket.
    pub fn builder(rlpx_tcp_socket: SocketAddr) -> ConfigBuilder {
        ConfigBuilder {
            discv5_config: None,
            bootstrap_nodes: HashSet::default(),
            fork: None,
            tcp_socket: rlpx_tcp_socket,
            other_enr_kv_pairs: Vec::new(),
            lookup_interval: None,
            bootstrap_lookup_interval: None,
            bootstrap_lookup_countdown: None,
            discovered_peer_filter: None,
        }
    }

    /// Inserts a new boot node to the list of boot nodes.
    pub fn insert_boot_node(&mut self, boot_node: BootNode) {
        self.bootstrap_nodes.insert(boot_node);
    }

    /// Inserts a new unsigned enode boot node to the list of boot nodes if it can be parsed, see
    /// also [`BootNode::from_unsigned`].
    pub fn insert_unsigned_boot_node(&mut self, node_record: NodeRecord) {
        let _ = BootNode::from_unsigned(node_record).map(|node| self.insert_boot_node(node));
    }

    /// Extends the list of boot nodes with a list of enode boot nodes if they can be parsed.
    pub fn extend_unsigned_boot_nodes(
        &mut self,
        node_records: impl IntoIterator<Item = NodeRecord>,
    ) {
        for node_record in node_records {
            self.insert_unsigned_boot_node(node_record);
        }
    }

    /// Returns the discovery (UDP) socket contained in the [`discv5::Config`]. Returns the IPv6
    /// socket, if both IPv4 and v6 are configured. This socket will be advertised to peers in the
    /// local [`Enr`](discv5::enr::Enr).
    pub fn discovery_socket(&self) -> SocketAddr {
        match self.discv5_config.listen_config {
            ListenConfig::Ipv4 { ip, port } => (ip, port).into(),
            ListenConfig::Ipv6 { ip, port } => (ip, port).into(),
            ListenConfig::DualStack { ipv6, ipv6_port, .. } => (ipv6, ipv6_port).into(),
        }
    }

    /// Returns the `RLPx` (TCP) socket contained in the [`discv5::Config`]. This socket will be
    /// advertised to peers in the local [`Enr`](discv5::enr::Enr).
    pub const fn rlpx_socket(&self) -> &SocketAddr {
        &self.tcp_socket
    }
}

/// Returns the IPv4 discovery socket if one is configured.
pub const fn ipv4(listen_config: &ListenConfig) -> Option<SocketAddrV4> {
    match listen_config {
        ListenConfig::Ipv4 { ip, port } |
        ListenConfig::DualStack { ipv4: ip, ipv4_port: port, .. } => {
            Some(SocketAddrV4::new(*ip, *port))
        }
        ListenConfig::Ipv6 { .. } => None,
    }
}

/// Returns the IPv6 discovery socket if one is configured.
pub const fn ipv6(listen_config: &ListenConfig) -> Option<SocketAddrV6> {
    match listen_config {
        ListenConfig::Ipv4 { .. } => None,
        ListenConfig::Ipv6 { ip, port } |
        ListenConfig::DualStack { ipv6: ip, ipv6_port: port, .. } => {
            Some(SocketAddrV6::new(*ip, *port, 0, 0))
        }
    }
}

/// Returns the amended [`discv5::ListenConfig`] based on the `RLPx` IP address. The ENR is limited
/// to one IP address per IP version (atm, may become spec'd how to advertise different addresses).
/// The `RLPx` address overwrites the discv5 address w.r.t. IP version.
pub fn amend_listen_config_wrt_rlpx(
    listen_config: &ListenConfig,
    rlpx_addr: IpAddr,
) -> ListenConfig {
    let discv5_socket_ipv4 = ipv4(listen_config);
    let discv5_socket_ipv6 = ipv6(listen_config);

    let discv5_port_ipv4 =
        discv5_socket_ipv4.map(|socket| socket.port()).unwrap_or(DEFAULT_DISCOVERY_V5_PORT);
    let discv5_addr_ipv4 = discv5_socket_ipv4.map(|socket| *socket.ip());
    let discv5_port_ipv6 =
        discv5_socket_ipv6.map(|socket| socket.port()).unwrap_or(DEFAULT_DISCOVERY_V5_PORT);
    let discv5_addr_ipv6 = discv5_socket_ipv6.map(|socket| *socket.ip());

    let (discv5_socket_ipv4, discv5_socket_ipv6) = discv5_sockets_wrt_rlpx_addr(
        rlpx_addr,
        discv5_addr_ipv4,
        discv5_port_ipv4,
        discv5_addr_ipv6,
        discv5_port_ipv6,
    );

    ListenConfig::from_two_sockets(discv5_socket_ipv4, discv5_socket_ipv6)
}

/// Returns the sockets that can be used for discv5 with respect to the `RLPx` address. ENR specs
/// only acknowledge one address per IP version.
pub fn discv5_sockets_wrt_rlpx_addr(
    rlpx_addr: IpAddr,
    discv5_addr_ipv4: Option<Ipv4Addr>,
    discv5_port_ipv4: u16,
    discv5_addr_ipv6: Option<Ipv6Addr>,
    discv5_port_ipv6: u16,
) -> (Option<SocketAddrV4>, Option<SocketAddrV6>) {
    match rlpx_addr {
        IpAddr::V4(rlpx_addr) => {
            let discv5_socket_ipv6 =
                discv5_addr_ipv6.map(|ip| SocketAddrV6::new(ip, discv5_port_ipv6, 0, 0));

            if let Some(discv5_addr) = discv5_addr_ipv4 {
                warn!(target: "discv5",
                    %discv5_addr,
                    %rlpx_addr,
                    "Overwriting discv5 IPv4 address with RLPx IPv4 address, limited to one advertised IP address per IP version"
                );
            }

            // overwrite discv5 ipv4 addr with RLPx address. this is since there is no
            // spec'd way to advertise a different address for rlpx and discovery in the
            // ENR.
            (Some(SocketAddrV4::new(rlpx_addr, discv5_port_ipv4)), discv5_socket_ipv6)
        }
        IpAddr::V6(rlpx_addr) => {
            let discv5_socket_ipv4 =
                discv5_addr_ipv4.map(|ip| SocketAddrV4::new(ip, discv5_port_ipv4));

            if let Some(discv5_addr) = discv5_addr_ipv6 {
                warn!(target: "discv5",
                    %discv5_addr,
                    %rlpx_addr,
                    "Overwriting discv5 IPv6 address with RLPx IPv6 address, limited to one advertised IP address per IP version"
                );
            }

            // overwrite discv5 ipv6 addr with RLPx address. this is since there is no
            // spec'd way to advertise a different address for rlpx and discovery in the
            // ENR.
            (discv5_socket_ipv4, Some(SocketAddrV6::new(rlpx_addr, discv5_port_ipv6, 0, 0)))
        }
    }
}

/// A boot node can be added either as a string in either 'enode' URL scheme or serialized from
/// [`Enr`](discv5::Enr) type.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Display)]
pub enum BootNode {
    /// An unsigned node record.
    #[display("{_0}")]
    Enode(Multiaddr),
    /// A signed node record.
    #[display("{_0:?}")]
    Enr(discv5::Enr),
}

impl BootNode {
    /// Parses a [`NodeRecord`] and serializes according to CL format. Note: [`discv5`] is
    /// originally a CL library hence needs this format to add the node.
    pub fn from_unsigned(node_record: NodeRecord) -> Result<Self, secp256k1::Error> {
        let NodeRecord { address, udp_port, id, .. } = node_record;
        let mut multi_address = Multiaddr::empty();
        match address {
            IpAddr::V4(ip) => multi_address.push(Protocol::Ip4(ip)),
            IpAddr::V6(ip) => multi_address.push(Protocol::Ip6(ip)),
        }

        multi_address.push(Protocol::Udp(udp_port));
        let id = discv4_id_to_multiaddr_id(id)?;
        multi_address.push(Protocol::P2p(id));

        Ok(Self::Enode(multi_address))
    }
}

#[cfg(test)]
mod test {
    use std::net::SocketAddrV4;

    use alloy_primitives::hex;

    use super::*;

    const MULTI_ADDRESSES: &str = "/ip4/184.72.129.189/udp/30301/p2p/16Uiu2HAmSG2hdLwyQHQmG4bcJBgD64xnW63WMTLcrNq6KoZREfGb,/ip4/3.231.11.52/udp/30301/p2p/16Uiu2HAmMy4V8bi3XP7KDfSLQcLACSvTLroRRwEsTyFUKo8NCkkp,/ip4/54.198.153.150/udp/30301/p2p/16Uiu2HAmSVsb7MbRf1jg3Dvd6a3n5YNqKQwn1fqHCFgnbqCsFZKe,/ip4/3.220.145.177/udp/30301/p2p/16Uiu2HAm74pBDGdQ84XCZK27GRQbGFFwQ7RsSqsPwcGmCR3Cwn3B,/ip4/3.231.138.188/udp/30301/p2p/16Uiu2HAmMnTiJwgFtSVGV14ZNpwAvS1LUoF4pWWeNtURuV6C3zYB";
    const BOOT_NODES_OP_MAINNET_AND_BASE_MAINNET: &[&str] = &[
        "enode://ca2774c3c401325850b2477fd7d0f27911efbf79b1e8b335066516e2bd8c4c9e0ba9696a94b1cb030a88eac582305ff55e905e64fb77fe0edcd70a4e5296d3ec@34.65.175.185:30305",
        "enode://dd751a9ef8912be1bfa7a5e34e2c3785cc5253110bd929f385e07ba7ac19929fb0e0c5d93f77827291f4da02b2232240fbc47ea7ce04c46e333e452f8656b667@34.65.107.0:30305",
        "enode://c5d289b56a77b6a2342ca29956dfd07aadf45364dde8ab20d1dc4efd4d1bc6b4655d902501daea308f4d8950737a4e93a4dfedd17b49cd5760ffd127837ca965@34.65.202.239:30305",
        "enode://87a32fd13bd596b2ffca97020e31aef4ddcc1bbd4b95bb633d16c1329f654f34049ed240a36b449fda5e5225d70fe40bc667f53c304b71f8e68fc9d448690b51@3.231.138.188:30301",
        "enode://ca21ea8f176adb2e229ce2d700830c844af0ea941a1d8152a9513b966fe525e809c3a6c73a2c18a12b74ed6ec4380edf91662778fe0b79f6a591236e49e176f9@184.72.129.189:30301",
        "enode://acf4507a211ba7c1e52cdf4eef62cdc3c32e7c9c47998954f7ba024026f9a6b2150cd3f0b734d9c78e507ab70d59ba61dfe5c45e1078c7ad0775fb251d7735a2@3.220.145.177:30301",
        "enode://8a5a5006159bf079d06a04e5eceab2a1ce6e0f721875b2a9c96905336219dbe14203d38f70f3754686a6324f786c2f9852d8c0dd3adac2d080f4db35efc678c5@3.231.11.52:30301",
        "enode://cdadbe835308ad3557f9a1de8db411da1a260a98f8421d62da90e71da66e55e98aaa8e90aa7ce01b408a54e4bd2253d701218081ded3dbe5efbbc7b41d7cef79@54.198.153.150:30301"
    ];

    #[test]
    fn parse_boot_nodes() {
        const OP_SEPOLIA_CL_BOOTNODES: &str ="enr:-J64QBwRIWAco7lv6jImSOjPU_W266lHXzpAS5YOh7WmgTyBZkgLgOwo_mxKJq3wz2XRbsoBItbv1dCyjIoNq67mFguGAYrTxM42gmlkgnY0gmlwhBLSsHKHb3BzdGFja4S0lAUAiXNlY3AyNTZrMaEDmoWSi8hcsRpQf2eJsNUx-sqv6fH4btmo2HsAzZFAKnKDdGNwgiQGg3VkcIIkBg,enr:-J64QFa3qMsONLGphfjEkeYyF6Jkil_jCuJmm7_a42ckZeUQGLVzrzstZNb1dgBp1GGx9bzImq5VxJLP-BaptZThGiWGAYrTytOvgmlkgnY0gmlwhGsV-zeHb3BzdGFja4S0lAUAiXNlY3AyNTZrMaEDahfSECTIS_cXyZ8IyNf4leANlZnrsMEWTkEYxf4GMCmDdGNwgiQGg3VkcIIkBg";

        let config = Config::builder((Ipv4Addr::UNSPECIFIED, 30303).into())
            .add_cl_serialized_signed_boot_nodes(OP_SEPOLIA_CL_BOOTNODES)
            .build();

        let socket_1 = "18.210.176.114:9222".parse::<SocketAddrV4>().unwrap();
        let socket_2 = "107.21.251.55:9222".parse::<SocketAddrV4>().unwrap();

        for node in config.bootstrap_nodes {
            let BootNode::Enr(node) = node else { panic!() };
            assert!(
                socket_1 == node.udp4_socket().unwrap() && socket_1 == node.tcp4_socket().unwrap() ||
                    socket_2 == node.udp4_socket().unwrap() &&
                        socket_2 == node.tcp4_socket().unwrap()
            );
            assert_eq!("84b4940500", hex::encode(node.get_raw_rlp("opstack").unwrap()));
        }
    }

    #[test]
    fn parse_enodes() {
        let config = Config::builder((Ipv4Addr::UNSPECIFIED, 30303).into())
            .add_serialized_unsigned_boot_nodes(BOOT_NODES_OP_MAINNET_AND_BASE_MAINNET)
            .build();

        let bootstrap_nodes =
            config.bootstrap_nodes.into_iter().map(|node| format!("{node}")).collect::<Vec<_>>();

        for node in MULTI_ADDRESSES.split(&[',']) {
            assert!(bootstrap_nodes.contains(&node.to_string()));
        }
    }

    #[test]
    fn overwrite_ipv4_addr() {
        let rlpx_addr: Ipv4Addr = "192.168.0.1".parse().unwrap();

        let listen_config = DEFAULT_DISCOVERY_V5_LISTEN_CONFIG;

        let amended_config = amend_listen_config_wrt_rlpx(&listen_config, rlpx_addr.into());

        let config_socket_ipv4 = ipv4(&amended_config).unwrap();

        assert_eq!(*config_socket_ipv4.ip(), rlpx_addr);
        assert_eq!(config_socket_ipv4.port(), DEFAULT_DISCOVERY_V5_PORT);
        assert_eq!(ipv6(&amended_config), ipv6(&listen_config));
    }

    #[test]
    fn overwrite_ipv6_addr() {
        let rlpx_addr: Ipv6Addr = "fe80::1".parse().unwrap();

        let listen_config = DEFAULT_DISCOVERY_V5_LISTEN_CONFIG;

        let amended_config = amend_listen_config_wrt_rlpx(&listen_config, rlpx_addr.into());

        let config_socket_ipv6 = ipv6(&amended_config).unwrap();

        assert_eq!(*config_socket_ipv6.ip(), rlpx_addr);
        assert_eq!(config_socket_ipv6.port(), DEFAULT_DISCOVERY_V5_PORT);
        assert_eq!(ipv4(&amended_config), ipv4(&listen_config));
    }
}

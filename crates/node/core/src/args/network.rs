//! clap [Args](clap::Args) for network related arguments.

use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6},
    ops::Not,
    path::PathBuf,
};

use clap::Args;
use reth_chainspec::EthChainSpec;
use reth_config::Config;
use reth_discv4::{NodeRecord, DEFAULT_DISCOVERY_ADDR, DEFAULT_DISCOVERY_PORT};
use reth_discv5::{
    discv5::ListenConfig, DEFAULT_COUNT_BOOTSTRAP_LOOKUPS, DEFAULT_DISCOVERY_V5_PORT,
    DEFAULT_SECONDS_BOOTSTRAP_LOOKUP_INTERVAL, DEFAULT_SECONDS_LOOKUP_INTERVAL,
};
use reth_net_nat::{NatResolver, DEFAULT_NET_IF_NAME};
use reth_network::{
    transactions::{
        constants::{
            tx_fetcher::{
                DEFAULT_MAX_CAPACITY_CACHE_PENDING_FETCH, DEFAULT_MAX_COUNT_CONCURRENT_REQUESTS,
                DEFAULT_MAX_COUNT_CONCURRENT_REQUESTS_PER_PEER,
            },
            tx_manager::{
                DEFAULT_MAX_COUNT_PENDING_POOL_IMPORTS, DEFAULT_MAX_COUNT_TRANSACTIONS_SEEN_BY_PEER,
            },
        },
        TransactionFetcherConfig, TransactionsManagerConfig,
        DEFAULT_SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESP_ON_PACK_GET_POOLED_TRANSACTIONS_REQ,
        SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE,
    },
    HelloMessageWithProtocols, NetworkConfigBuilder, SessionsConfig,
};
use reth_network_peers::{mainnet_nodes, TrustedPeer};
use secp256k1::SecretKey;
use tracing::error;

use crate::version::P2P_CLIENT_VERSION;

/// Parameters for configuring the network more granularity via CLI
#[derive(Debug, Clone, Args, PartialEq, Eq)]
#[command(next_help_heading = "Networking")]
pub struct NetworkArgs {
    /// Arguments to setup discovery service.
    #[command(flatten)]
    pub discovery: DiscoveryArgs,

    #[allow(clippy::doc_markdown)]
    /// Comma separated enode URLs of trusted peers for P2P connections.
    ///
    /// --trusted-peers enode://abcd@192.168.0.1:30303
    #[arg(long, value_delimiter = ',')]
    pub trusted_peers: Vec<TrustedPeer>,

    /// Connect to or accept from trusted peers only
    #[arg(long)]
    pub trusted_only: bool,

    /// Comma separated enode URLs for P2P discovery bootstrap.
    ///
    /// Will fall back to a network-specific default if not specified.
    #[arg(long, value_delimiter = ',')]
    pub bootnodes: Option<Vec<TrustedPeer>>,

    /// Amount of DNS resolution requests retries to perform when peering.
    #[arg(long, default_value_t = 0)]
    pub dns_retries: usize,

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

    /// NAT resolution method (any|none|upnp|publicip|extip:\<IP\>)
    #[arg(long, default_value = "any")]
    pub nat: NatResolver,

    /// Network listening address
    #[arg(long = "addr", value_name = "ADDR", default_value_t = DEFAULT_DISCOVERY_ADDR)]
    pub addr: IpAddr,

    /// Network listening port
    #[arg(long = "port", value_name = "PORT", default_value_t = DEFAULT_DISCOVERY_PORT)]
    pub port: u16,

    /// Maximum number of outbound requests. default: 100
    #[arg(long)]
    pub max_outbound_peers: Option<usize>,

    /// Maximum number of inbound requests. default: 30
    #[arg(long)]
    pub max_inbound_peers: Option<usize>,

    /// Max concurrent `GetPooledTransactions` requests.
    #[arg(long = "max-tx-reqs", value_name = "COUNT", default_value_t = DEFAULT_MAX_COUNT_CONCURRENT_REQUESTS, verbatim_doc_comment)]
    pub max_concurrent_tx_requests: u32,

    /// Max concurrent `GetPooledTransactions` requests per peer.
    #[arg(long = "max-tx-reqs-peer", value_name = "COUNT", default_value_t = DEFAULT_MAX_COUNT_CONCURRENT_REQUESTS_PER_PEER, verbatim_doc_comment)]
    pub max_concurrent_tx_requests_per_peer: u8,

    /// Max number of seen transactions to remember per peer.
    ///
    /// Default is 320 transaction hashes.
    #[arg(long = "max-seen-tx-history", value_name = "COUNT", default_value_t = DEFAULT_MAX_COUNT_TRANSACTIONS_SEEN_BY_PEER, verbatim_doc_comment)]
    pub max_seen_tx_history: u32,

    #[arg(long = "max-pending-imports", value_name = "COUNT", default_value_t = DEFAULT_MAX_COUNT_PENDING_POOL_IMPORTS, verbatim_doc_comment)]
    /// Max number of transactions to import concurrently.
    pub max_pending_pool_imports: usize,

    /// Experimental, for usage in research. Sets the max accumulated byte size of transactions
    /// to pack in one response.
    /// Spec'd at 2MiB.
    #[arg(long = "pooled-tx-response-soft-limit", value_name = "BYTES", default_value_t = SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE, verbatim_doc_comment)]
    pub soft_limit_byte_size_pooled_transactions_response: usize,

    /// Experimental, for usage in research. Sets the max accumulated byte size of transactions to
    /// request in one request.
    ///
    /// Since `RLPx` protocol version 68, the byte size of a transaction is shared as metadata in a
    /// transaction announcement (see `RLPx` specs). This allows a node to request a specific size
    /// response.
    ///
    /// By default, nodes request only 128 KiB worth of transactions, but should a peer request
    /// more, up to 2 MiB, a node will answer with more than 128 KiB.
    ///
    /// Default is 128 KiB.
    #[arg(long = "pooled-tx-pack-soft-limit", value_name = "BYTES", default_value_t = DEFAULT_SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESP_ON_PACK_GET_POOLED_TRANSACTIONS_REQ, verbatim_doc_comment)]
    pub soft_limit_byte_size_pooled_transactions_response_on_pack_request: usize,

    /// Max capacity of cache of hashes for transactions pending fetch.
    #[arg(long = "max-tx-pending-fetch", value_name = "COUNT", default_value_t = DEFAULT_MAX_CAPACITY_CACHE_PENDING_FETCH, verbatim_doc_comment)]
    pub max_capacity_cache_txns_pending_fetch: u32,

    /// Name of network interface used to communicate with peers.
    ///
    /// If flag is set, but no value is passed, the default interface for docker `eth0` is tried.
    #[arg(long = "net-if.experimental", conflicts_with = "addr", value_name = "IF_NAME")]
    pub net_if: Option<String>,
}

impl NetworkArgs {
    /// Returns the resolved IP address.
    pub fn resolved_addr(&self) -> IpAddr {
        if let Some(ref if_name) = self.net_if {
            let if_name = if if_name.is_empty() { DEFAULT_NET_IF_NAME } else { if_name };
            return match reth_net_nat::net_if::resolve_net_if_ip(if_name) {
                Ok(addr) => addr,
                Err(err) => {
                    error!(target: "reth::cli",
                        if_name,
                        %err,
                        "Failed to read network interface IP"
                    );

                    DEFAULT_DISCOVERY_ADDR
                }
            };
        }

        self.addr
    }

    /// Returns the resolved bootnodes if any are provided.
    pub fn resolved_bootnodes(&self) -> Option<Vec<NodeRecord>> {
        self.bootnodes.clone().map(|bootnodes| {
            bootnodes.into_iter().filter_map(|node| node.resolve_blocking().ok()).collect()
        })
    }

    /// Build a [`NetworkConfigBuilder`] from a [`Config`] and a [`EthChainSpec`], in addition to
    /// the values in this option struct.
    ///
    /// The `default_peers_file` will be used as the default location to store the persistent peers
    /// file if `no_persist_peers` is false, and there is no provided `peers_file`.
    ///
    /// Configured Bootnodes are prioritized, if unset, the chain spec bootnodes are used
    /// Priority order for bootnodes configuration:
    /// 1. --bootnodes flag
    /// 2. Network preset flags (e.g. --holesky)
    /// 3. default to mainnet nodes
    pub fn network_config(
        &self,
        config: &Config,
        chain_spec: impl EthChainSpec,
        secret_key: SecretKey,
        default_peers_file: PathBuf,
    ) -> NetworkConfigBuilder {
        let addr = self.resolved_addr();
        let chain_bootnodes = self
            .resolved_bootnodes()
            .unwrap_or_else(|| chain_spec.bootnodes().unwrap_or_else(mainnet_nodes));
        let peers_file = self.peers_file.clone().unwrap_or(default_peers_file);

        // Configure peer connections
        let peers_config = config
            .peers
            .clone()
            .with_max_inbound_opt(self.max_inbound_peers)
            .with_max_outbound_opt(self.max_outbound_peers);

        // Configure transactions manager
        let transactions_manager_config = TransactionsManagerConfig {
            transaction_fetcher_config: TransactionFetcherConfig::new(
                self.max_concurrent_tx_requests,
                self.max_concurrent_tx_requests_per_peer,
                self.soft_limit_byte_size_pooled_transactions_response,
                self.soft_limit_byte_size_pooled_transactions_response_on_pack_request,
                self.max_capacity_cache_txns_pending_fetch,
            ),
            max_transactions_seen_by_peer_history: self.max_seen_tx_history,
            propagation_mode: Default::default(),
        };

        // Configure basic network stack
        NetworkConfigBuilder::new(secret_key)
            .peer_config(config.peers_config_with_basic_nodes_from_file(
                self.persistent_peers_file(peers_file).as_deref(),
            ))
            .external_ip_resolver(self.nat)
            .sessions_config(
                SessionsConfig::default().with_upscaled_event_buffer(peers_config.max_peers()),
            )
            .peer_config(peers_config)
            .boot_nodes(chain_bootnodes.clone())
            .transactions_manager_config(transactions_manager_config)
            // Configure node identity
            .apply(|builder| {
                let peer_id = builder.get_peer_id();
                builder.hello_message(
                    HelloMessageWithProtocols::builder(peer_id)
                        .client_version(&self.identity)
                        .build(),
                )
            })
            // apply discovery settings
            .apply(|builder| {
                let rlpx_socket = (addr, self.port).into();
                self.discovery.apply_to_builder(builder, rlpx_socket, chain_bootnodes)
            })
            .listener_addr(SocketAddr::new(
                addr, // set discovery port based on instance number
                self.port,
            ))
            .discovery_addr(SocketAddr::new(
                self.discovery.addr,
                // set discovery port based on instance number
                self.discovery.port,
            ))
    }

    /// If `no_persist_peers` is false then this returns the path to the persistent peers file path.
    pub fn persistent_peers_file(&self, peers_file: PathBuf) -> Option<PathBuf> {
        self.no_persist_peers.not().then_some(peers_file)
    }

    /// Sets the p2p port to zero, to allow the OS to assign a random unused port when
    /// the network components bind to a socket.
    pub const fn with_unused_p2p_port(mut self) -> Self {
        self.port = 0;
        self
    }

    /// Sets the p2p and discovery ports to zero, allowing the OD to assign a random unused port
    /// when network components bind to sockets.
    pub const fn with_unused_ports(mut self) -> Self {
        self = self.with_unused_p2p_port();
        self.discovery = self.discovery.with_unused_discovery_port();
        self
    }

    /// Change networking port numbers based on the instance number.
    /// Ports are updated to `previous_value + instance - 1`
    ///
    /// # Panics
    /// Warning: if `instance` is zero in debug mode, this will panic.
    pub fn adjust_instance_ports(&mut self, instance: u16) {
        debug_assert_ne!(instance, 0, "instance must be non-zero");
        self.port += instance - 1;
        self.discovery.adjust_instance_ports(instance);
    }

    /// Resolve all trusted peers at once
    pub async fn resolve_trusted_peers(&self) -> Result<Vec<NodeRecord>, std::io::Error> {
        futures::future::try_join_all(
            self.trusted_peers.iter().map(|peer| async move { peer.resolve().await }),
        )
        .await
    }
}

impl Default for NetworkArgs {
    fn default() -> Self {
        Self {
            discovery: DiscoveryArgs::default(),
            trusted_peers: vec![],
            trusted_only: false,
            bootnodes: None,
            dns_retries: 0,
            peers_file: None,
            identity: P2P_CLIENT_VERSION.to_string(),
            p2p_secret_key: None,
            no_persist_peers: false,
            nat: NatResolver::Any,
            addr: DEFAULT_DISCOVERY_ADDR,
            port: DEFAULT_DISCOVERY_PORT,
            max_outbound_peers: None,
            max_inbound_peers: None,
            max_concurrent_tx_requests: DEFAULT_MAX_COUNT_CONCURRENT_REQUESTS,
            max_concurrent_tx_requests_per_peer: DEFAULT_MAX_COUNT_CONCURRENT_REQUESTS_PER_PEER,
            soft_limit_byte_size_pooled_transactions_response:
                SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE,
            soft_limit_byte_size_pooled_transactions_response_on_pack_request: DEFAULT_SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESP_ON_PACK_GET_POOLED_TRANSACTIONS_REQ,
            max_pending_pool_imports: DEFAULT_MAX_COUNT_PENDING_POOL_IMPORTS,
            max_seen_tx_history: DEFAULT_MAX_COUNT_TRANSACTIONS_SEEN_BY_PEER,
            max_capacity_cache_txns_pending_fetch: DEFAULT_MAX_CAPACITY_CACHE_PENDING_FETCH,
            net_if: None,
        }
    }
}

/// Arguments to setup discovery
#[derive(Debug, Clone, Args, PartialEq, Eq)]
pub struct DiscoveryArgs {
    /// Disable the discovery service.
    #[arg(short, long, default_value_if("dev", "true", "true"))]
    pub disable_discovery: bool,

    /// Disable the DNS discovery.
    #[arg(long, conflicts_with = "disable_discovery")]
    pub disable_dns_discovery: bool,

    /// Disable Discv4 discovery.
    #[arg(long, conflicts_with = "disable_discovery")]
    pub disable_discv4_discovery: bool,

    /// Enable Discv5 discovery.
    #[arg(long, conflicts_with = "disable_discovery")]
    pub enable_discv5_discovery: bool,

    /// Disable Nat discovery.
    #[arg(long, conflicts_with = "disable_discovery")]
    pub disable_nat: bool,

    /// The UDP address to use for devp2p peer discovery version 4.
    #[arg(id = "discovery.addr", long = "discovery.addr", value_name = "DISCOVERY_ADDR", default_value_t = DEFAULT_DISCOVERY_ADDR)]
    pub addr: IpAddr,

    /// The UDP port to use for devp2p peer discovery version 4.
    #[arg(id = "discovery.port", long = "discovery.port", value_name = "DISCOVERY_PORT", default_value_t = DEFAULT_DISCOVERY_PORT)]
    pub port: u16,

    /// The UDP IPv4 address to use for devp2p peer discovery version 5. Overwritten by `RLPx`
    /// address, if it's also IPv4.
    #[arg(id = "discovery.v5.addr", long = "discovery.v5.addr", value_name = "DISCOVERY_V5_ADDR", default_value = None)]
    pub discv5_addr: Option<Ipv4Addr>,

    /// The UDP IPv6 address to use for devp2p peer discovery version 5. Overwritten by `RLPx`
    /// address, if it's also IPv6.
    #[arg(id = "discovery.v5.addr.ipv6", long = "discovery.v5.addr.ipv6", value_name = "DISCOVERY_V5_ADDR_IPV6", default_value = None)]
    pub discv5_addr_ipv6: Option<Ipv6Addr>,

    /// The UDP IPv4 port to use for devp2p peer discovery version 5. Not used unless `--addr` is
    /// IPv4, or `--discovery.v5.addr` is set.
    #[arg(id = "discovery.v5.port", long = "discovery.v5.port", value_name = "DISCOVERY_V5_PORT",
    default_value_t = DEFAULT_DISCOVERY_V5_PORT)]
    pub discv5_port: u16,

    /// The UDP IPv6 port to use for devp2p peer discovery version 5. Not used unless `--addr` is
    /// IPv6, or `--discovery.addr.ipv6` is set.
    #[arg(id = "discovery.v5.port.ipv6", long = "discovery.v5.port.ipv6", value_name = "DISCOVERY_V5_PORT_IPV6",
    default_value = None, default_value_t = DEFAULT_DISCOVERY_V5_PORT)]
    pub discv5_port_ipv6: u16,

    /// The interval in seconds at which to carry out periodic lookup queries, for the whole
    /// run of the program.
    #[arg(id = "discovery.v5.lookup-interval", long = "discovery.v5.lookup-interval", value_name = "DISCOVERY_V5_LOOKUP_INTERVAL", default_value_t = DEFAULT_SECONDS_LOOKUP_INTERVAL)]
    pub discv5_lookup_interval: u64,

    /// The interval in seconds at which to carry out boost lookup queries, for a fixed number of
    /// times, at bootstrap.
    #[arg(id = "discovery.v5.bootstrap.lookup-interval", long = "discovery.v5.bootstrap.lookup-interval", value_name = "DISCOVERY_V5_BOOTSTRAP_LOOKUP_INTERVAL",
        default_value_t = DEFAULT_SECONDS_BOOTSTRAP_LOOKUP_INTERVAL)]
    pub discv5_bootstrap_lookup_interval: u64,

    /// The number of times to carry out boost lookup queries at bootstrap.
    #[arg(id = "discovery.v5.bootstrap.lookup-countdown", long = "discovery.v5.bootstrap.lookup-countdown", value_name = "DISCOVERY_V5_BOOTSTRAP_LOOKUP_COUNTDOWN",
        default_value_t = DEFAULT_COUNT_BOOTSTRAP_LOOKUPS)]
    pub discv5_bootstrap_lookup_countdown: u64,
}

impl DiscoveryArgs {
    /// Apply the discovery settings to the given [`NetworkConfigBuilder`]
    pub fn apply_to_builder(
        &self,
        mut network_config_builder: NetworkConfigBuilder,
        rlpx_tcp_socket: SocketAddr,
        boot_nodes: impl IntoIterator<Item = NodeRecord>,
    ) -> NetworkConfigBuilder {
        if self.disable_discovery || self.disable_dns_discovery {
            network_config_builder = network_config_builder.disable_dns_discovery();
        }

        if self.disable_discovery || self.disable_discv4_discovery {
            network_config_builder = network_config_builder.disable_discv4_discovery();
        }

        if self.disable_discovery || self.disable_nat {
            network_config_builder = network_config_builder.disable_nat();
        }

        if !self.disable_discovery && self.enable_discv5_discovery {
            network_config_builder = network_config_builder
                .discovery_v5(self.discovery_v5_builder(rlpx_tcp_socket, boot_nodes));
        }

        network_config_builder
    }

    /// Creates a [`reth_discv5::ConfigBuilder`] filling it with the values from this struct.
    pub fn discovery_v5_builder(
        &self,
        rlpx_tcp_socket: SocketAddr,
        boot_nodes: impl IntoIterator<Item = NodeRecord>,
    ) -> reth_discv5::ConfigBuilder {
        let Self {
            discv5_addr,
            discv5_addr_ipv6,
            discv5_port,
            discv5_port_ipv6,
            discv5_lookup_interval,
            discv5_bootstrap_lookup_interval,
            discv5_bootstrap_lookup_countdown,
            ..
        } = self;

        // Use rlpx address if none given
        let discv5_addr_ipv4 = discv5_addr.or(match rlpx_tcp_socket {
            SocketAddr::V4(addr) => Some(*addr.ip()),
            SocketAddr::V6(_) => None,
        });
        let discv5_addr_ipv6 = discv5_addr_ipv6.or(match rlpx_tcp_socket {
            SocketAddr::V4(_) => None,
            SocketAddr::V6(addr) => Some(*addr.ip()),
        });

        reth_discv5::Config::builder(rlpx_tcp_socket)
            .discv5_config(
                reth_discv5::discv5::ConfigBuilder::new(ListenConfig::from_two_sockets(
                    discv5_addr_ipv4.map(|addr| SocketAddrV4::new(addr, *discv5_port)),
                    discv5_addr_ipv6.map(|addr| SocketAddrV6::new(addr, *discv5_port_ipv6, 0, 0)),
                ))
                .build(),
            )
            .add_unsigned_boot_nodes(boot_nodes)
            .lookup_interval(*discv5_lookup_interval)
            .bootstrap_lookup_interval(*discv5_bootstrap_lookup_interval)
            .bootstrap_lookup_countdown(*discv5_bootstrap_lookup_countdown)
    }

    /// Set the discovery port to zero, to allow the OS to assign a random unused port when
    /// discovery binds to the socket.
    pub const fn with_unused_discovery_port(mut self) -> Self {
        self.port = 0;
        self
    }

    /// Change networking port numbers based on the instance number.
    /// Ports are updated to `previous_value + instance - 1`
    ///
    /// # Panics
    /// Warning: if `instance` is zero in debug mode, this will panic.
    pub fn adjust_instance_ports(&mut self, instance: u16) {
        debug_assert_ne!(instance, 0, "instance must be non-zero");
        self.port += instance - 1;
        self.discv5_port += instance - 1;
        self.discv5_port_ipv6 += instance - 1;
    }
}

impl Default for DiscoveryArgs {
    fn default() -> Self {
        Self {
            disable_discovery: false,
            disable_dns_discovery: false,
            disable_discv4_discovery: false,
            enable_discv5_discovery: false,
            disable_nat: false,
            addr: DEFAULT_DISCOVERY_ADDR,
            port: DEFAULT_DISCOVERY_PORT,
            discv5_addr: None,
            discv5_addr_ipv6: None,
            discv5_port: DEFAULT_DISCOVERY_V5_PORT,
            discv5_port_ipv6: DEFAULT_DISCOVERY_V5_PORT,
            discv5_lookup_interval: DEFAULT_SECONDS_LOOKUP_INTERVAL,
            discv5_bootstrap_lookup_interval: DEFAULT_SECONDS_BOOTSTRAP_LOOKUP_INTERVAL,
            discv5_bootstrap_lookup_countdown: DEFAULT_COUNT_BOOTSTRAP_LOOKUPS,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    /// A helper type to parse Args more easily
    #[derive(Parser)]
    struct CommandParser<T: Args> {
        #[command(flatten)]
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

    #[test]
    fn parse_peer_args() {
        let args =
            CommandParser::<NetworkArgs>::parse_from(["reth", "--max-outbound-peers", "50"]).args;
        assert_eq!(args.max_outbound_peers, Some(50));
        assert_eq!(args.max_inbound_peers, None);

        let args = CommandParser::<NetworkArgs>::parse_from([
            "reth",
            "--max-outbound-peers",
            "75",
            "--max-inbound-peers",
            "15",
        ])
        .args;
        assert_eq!(args.max_outbound_peers, Some(75));
        assert_eq!(args.max_inbound_peers, Some(15));
    }

    #[test]
    fn parse_trusted_peer_args() {
        let args =
            CommandParser::<NetworkArgs>::parse_from([
            "reth",
            "--trusted-peers",
            "enode://d860a01f9722d78051619d1e2351aba3f43f943f6f00718d1b9baa4101932a1f5011f16bb2b1bb35db20d6fe28fa0bf09636d26a87d31de9ec6203eeedb1f666@18.138.108.67:30303,enode://22a8232c3abc76a16ae9d6c3b164f98775fe226f0917b0ca871128a74a8e9630b458460865bab457221f1d448dd9791d24c4e5d88786180ac185df813a68d4de@3.209.45.79:30303"
        ])
        .args;

        assert_eq!(
            args.trusted_peers,
            vec![
            "enode://d860a01f9722d78051619d1e2351aba3f43f943f6f00718d1b9baa4101932a1f5011f16bb2b1bb35db20d6fe28fa0bf09636d26a87d31de9ec6203eeedb1f666@18.138.108.67:30303".parse().unwrap(),
            "enode://22a8232c3abc76a16ae9d6c3b164f98775fe226f0917b0ca871128a74a8e9630b458460865bab457221f1d448dd9791d24c4e5d88786180ac185df813a68d4de@3.209.45.79:30303".parse().unwrap()
            ]
        );
    }

    #[test]
    fn parse_retry_strategy_args() {
        let tests = vec![0, 10];

        for retries in tests {
            let args = CommandParser::<NetworkArgs>::parse_from([
                "reth",
                "--dns-retries",
                retries.to_string().as_str(),
            ])
            .args;

            assert_eq!(args.dns_retries, retries);
        }
    }

    #[cfg(not(feature = "optimism"))]
    #[test]
    fn network_args_default_sanity_test() {
        let default_args = NetworkArgs::default();
        let args = CommandParser::<NetworkArgs>::parse_from(["reth"]).args;

        assert_eq!(args, default_args);
    }
}

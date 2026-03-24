//! clap [Args](clap::Args) for network related arguments.

use alloy_eips::BlockNumHash;
use alloy_primitives::B256;
use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6},
    ops::Not,
    path::PathBuf,
};

use crate::version::version_metadata;
use clap::Args;
use reth_chainspec::EthChainSpec;
use reth_cli_util::{get_secret_key, load_secret_key::SecretKeyError};
use reth_config::Config;
use reth_discv4::{NodeRecord, DEFAULT_DISCOVERY_ADDR, DEFAULT_DISCOVERY_PORT};
use reth_discv5::{
    discv5::ListenConfig, DEFAULT_COUNT_BOOTSTRAP_LOOKUPS, DEFAULT_DISCOVERY_V5_PORT,
    DEFAULT_SECONDS_BOOTSTRAP_LOOKUP_INTERVAL, DEFAULT_SECONDS_LOOKUP_INTERVAL,
};
use reth_net_banlist::IpFilter;
use reth_net_nat::{NatResolver, DEFAULT_NET_IF_NAME};
use reth_network::{
    transactions::{
        config::{TransactionIngressPolicy, TransactionPropagationKind},
        constants::{
            tx_fetcher::{
                DEFAULT_MAX_CAPACITY_CACHE_PENDING_FETCH, DEFAULT_MAX_COUNT_CONCURRENT_REQUESTS,
                DEFAULT_MAX_COUNT_CONCURRENT_REQUESTS_PER_PEER,
            },
            tx_manager::{
                DEFAULT_MAX_COUNT_PENDING_POOL_IMPORTS, DEFAULT_MAX_COUNT_TRANSACTIONS_SEEN_BY_PEER,
            },
        },
        TransactionFetcherConfig, TransactionPropagationMode, TransactionsManagerConfig,
        DEFAULT_SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESP_ON_PACK_GET_POOLED_TRANSACTIONS_REQ,
        SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE,
    },
    HelloMessageWithProtocols, NetworkConfigBuilder, NetworkPrimitives,
};
use reth_network_peers::{mainnet_nodes, TrustedPeer};
use reth_tasks::Runtime;
use secp256k1::SecretKey;
use std::str::FromStr;
use tracing::error;

/// Parameters for configuring the network more granularity via CLI
#[derive(Debug, Clone, Args, PartialEq, Eq)]
#[command(next_help_heading = "Networking")]
pub struct NetworkArgs {
    /// Arguments to setup discovery service.
    #[command(flatten)]
    pub discovery: DiscoveryArgs,

    #[expect(clippy::doc_markdown)]
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
    #[arg(long, value_name = "IDENTITY", default_value = version_metadata().p2p_client_version.as_ref())]
    pub identity: String,

    /// Secret key to use for this node.
    ///
    /// This will also deterministically set the peer ID. If not specified, it will be set in the
    /// data dir for the chain being used.
    #[arg(long, value_name = "PATH", conflicts_with = "p2p_secret_key_hex")]
    pub p2p_secret_key: Option<PathBuf>,

    /// Hex encoded secret key to use for this node.
    ///
    /// This will also deterministically set the peer ID. Cannot be used together with
    /// `--p2p-secret-key`.
    #[arg(long, value_name = "HEX", conflicts_with = "p2p_secret_key")]
    pub p2p_secret_key_hex: Option<B256>,

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

    /// Maximum number of outbound peers. default: 100
    #[arg(long)]
    pub max_outbound_peers: Option<usize>,

    /// Maximum number of inbound peers. default: 30
    #[arg(long)]
    pub max_inbound_peers: Option<usize>,

    /// Maximum number of total peers (inbound + outbound).
    ///
    /// Splits peers using approximately 2:1 inbound:outbound ratio. Cannot be used together with
    /// `--max-outbound-peers` or `--max-inbound-peers`.
    #[arg(
        long,
        value_name = "COUNT",
        conflicts_with = "max_outbound_peers",
        conflicts_with = "max_inbound_peers"
    )]
    pub max_peers: Option<usize>,

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

    /// Transaction Propagation Policy
    ///
    /// The policy determines which peers transactions are gossiped to.
    #[arg(long = "tx-propagation-policy", default_value_t = TransactionPropagationKind::All)]
    pub tx_propagation_policy: TransactionPropagationKind,

    /// Transaction ingress policy
    ///
    /// Determines which peers' transactions are accepted over P2P.
    #[arg(long = "tx-ingress-policy", default_value_t = TransactionIngressPolicy::All)]
    pub tx_ingress_policy: TransactionIngressPolicy,

    /// Disable transaction pool gossip
    ///
    /// Disables gossiping of transactions in the mempool to peers. This can be omitted for
    /// personal nodes, though providers should always opt to enable this flag.
    #[arg(long = "disable-tx-gossip")]
    pub disable_tx_gossip: bool,

    /// Sets the transaction propagation mode by determining how new pending transactions are
    /// propagated to other peers in full.
    ///
    /// Examples: sqrt, all, max:10
    #[arg(
        long = "tx-propagation-mode",
        default_value = "sqrt",
        help = "Transaction propagation mode (sqrt, all, max:<number>)"
    )]
    pub propagation_mode: TransactionPropagationMode,

    /// Comma separated list of required block hashes or block number=hash pairs.
    /// Peers that don't have these blocks will be filtered out.
    /// Format: hash or `block_number=hash` (e.g., 23115201=0x1234...)
    #[arg(long = "required-block-hashes", value_delimiter = ',', value_parser = parse_block_num_hash)]
    pub required_block_hashes: Vec<BlockNumHash>,

    /// Optional network ID to override the chain specification's network ID for P2P connections
    #[arg(long)]
    pub network_id: Option<u64>,

    /// Restrict network communication to the given IP networks (CIDR masks).
    ///
    /// Comma separated list of CIDR network specifications.
    /// Only peers with IP addresses within these ranges will be allowed to connect.
    ///
    /// Example: --netrestrict "192.168.0.0/16,10.0.0.0/8"
    #[arg(long, value_name = "NETRESTRICT")]
    pub netrestrict: Option<String>,

    /// Enforce EIP-868 ENR fork ID validation for discovered peers.
    ///
    /// When enabled, peers discovered without a confirmed fork ID are not added to the peer set
    /// until their fork ID is verified via EIP-868 ENR request. This filters out peers from other
    /// networks that pollute the discovery table.
    #[arg(long)]
    pub enforce_enr_fork_id: bool,
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

    /// Returns the max inbound peers (2:1 ratio).
    pub fn resolved_max_inbound_peers(&self) -> Option<usize> {
        if let Some(max_peers) = self.max_peers {
            if max_peers == 0 {
                Some(0)
            } else {
                let outbound = (max_peers / 3).max(1);
                Some(max_peers.saturating_sub(outbound))
            }
        } else {
            self.max_inbound_peers
        }
    }

    /// Returns the max outbound peers (1:2 ratio).
    pub fn resolved_max_outbound_peers(&self) -> Option<usize> {
        if let Some(max_peers) = self.max_peers {
            if max_peers == 0 {
                Some(0)
            } else {
                Some((max_peers / 3).max(1))
            }
        } else {
            self.max_outbound_peers
        }
    }

    /// Configures and returns a `TransactionsManagerConfig` based on the current settings.
    pub const fn transactions_manager_config(&self) -> TransactionsManagerConfig {
        TransactionsManagerConfig {
            transaction_fetcher_config: TransactionFetcherConfig::new(
                self.max_concurrent_tx_requests,
                self.max_concurrent_tx_requests_per_peer,
                self.soft_limit_byte_size_pooled_transactions_response,
                self.soft_limit_byte_size_pooled_transactions_response_on_pack_request,
                self.max_capacity_cache_txns_pending_fetch,
            ),
            max_transactions_seen_by_peer_history: self.max_seen_tx_history,
            propagation_mode: self.propagation_mode,
            ingress_policy: self.tx_ingress_policy,
        }
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
    pub fn network_config<N: NetworkPrimitives>(
        &self,
        config: &Config,
        chain_spec: impl EthChainSpec,
        secret_key: SecretKey,
        default_peers_file: PathBuf,
        executor: Runtime,
    ) -> NetworkConfigBuilder<N> {
        let addr = self.resolved_addr();
        let chain_bootnodes = self
            .resolved_bootnodes()
            .unwrap_or_else(|| chain_spec.bootnodes().unwrap_or_else(mainnet_nodes));
        let peers_file = self.peers_file.clone().unwrap_or(default_peers_file);

        // Configure peer connections
        let ip_filter = self.ip_filter().unwrap_or_default();
        let peers_config = config
            .peers_config_with_basic_nodes_from_file(
                self.persistent_peers_file(peers_file).as_deref(),
            )
            .with_max_inbound_opt(self.resolved_max_inbound_peers())
            .with_max_outbound_opt(self.resolved_max_outbound_peers())
            .with_ip_filter(ip_filter)
            .with_enforce_enr_fork_id(self.enforce_enr_fork_id);

        // Configure basic network stack
        NetworkConfigBuilder::<N>::new(secret_key, executor)
            .external_ip_resolver(self.nat.clone())
            .sessions_config(
                config.sessions.clone().with_upscaled_event_buffer(peers_config.max_peers()),
            )
            .peer_config(peers_config)
            .boot_nodes(chain_bootnodes.clone())
            .transactions_manager_config(self.transactions_manager_config())
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
            .disable_tx_gossip(self.disable_tx_gossip)
            .required_block_hashes(self.required_block_hashes.clone())
            .network_id(self.network_id)
    }

    /// If `no_persist_peers` is false then this returns the path to the persistent peers file path.
    pub fn persistent_peers_file(&self, peers_file: PathBuf) -> Option<PathBuf> {
        self.no_persist_peers.not().then_some(peers_file)
    }

    /// Configures the [`DiscoveryArgs`].
    pub const fn with_discovery(mut self, discovery: DiscoveryArgs) -> Self {
        self.discovery = discovery;
        self
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

    /// Configures the [`NatResolver`]
    pub fn with_nat_resolver(mut self, nat: NatResolver) -> Self {
        self.nat = nat;
        self
    }

    /// Change networking port numbers based on the instance number, if provided.
    /// Ports are updated to `previous_value + instance - 1`
    ///
    /// # Panics
    /// Warning: if `instance` is zero in debug mode, this will panic.
    pub fn adjust_instance_ports(&mut self, instance: Option<u16>) {
        if let Some(instance) = instance {
            debug_assert_ne!(instance, 0, "instance must be non-zero");
            self.port += instance - 1;
            self.discovery.adjust_instance_ports(instance);
        }
    }

    /// Resolve all trusted peers at once
    pub async fn resolve_trusted_peers(&self) -> Result<Vec<NodeRecord>, std::io::Error> {
        futures::future::try_join_all(
            self.trusted_peers.iter().map(|peer| async move { peer.resolve().await }),
        )
        .await
    }

    /// Load the p2p secret key from the provided options.
    ///
    /// If `p2p_secret_key_hex` is provided, it will be used directly.
    /// If `p2p_secret_key` is provided, it will be loaded from the file.
    /// If neither is provided, the `default_secret_key_path` will be used.
    pub fn secret_key(
        &self,
        default_secret_key_path: PathBuf,
    ) -> Result<SecretKey, SecretKeyError> {
        if let Some(b256) = &self.p2p_secret_key_hex {
            // Use the B256 value directly (already validated as 32 bytes)
            SecretKey::from_slice(b256.as_slice()).map_err(SecretKeyError::SecretKeyDecodeError)
        } else {
            // Load from file (either provided path or default)
            let secret_key_path = self.p2p_secret_key.clone().unwrap_or(default_secret_key_path);
            get_secret_key(&secret_key_path)
        }
    }

    /// Creates an IP filter from the netrestrict argument.
    ///
    /// Returns an error if the CIDR format is invalid.
    pub fn ip_filter(&self) -> Result<IpFilter, ipnet::AddrParseError> {
        if let Some(netrestrict) = &self.netrestrict {
            IpFilter::from_cidr_string(netrestrict)
        } else {
            Ok(IpFilter::allow_all())
        }
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
            identity: version_metadata().p2p_client_version.to_string(),
            p2p_secret_key: None,
            p2p_secret_key_hex: None,
            no_persist_peers: false,
            nat: NatResolver::Any,
            addr: DEFAULT_DISCOVERY_ADDR,
            port: DEFAULT_DISCOVERY_PORT,
            max_outbound_peers: None,
            max_inbound_peers: None,
            max_peers: None,
            max_concurrent_tx_requests: DEFAULT_MAX_COUNT_CONCURRENT_REQUESTS,
            max_concurrent_tx_requests_per_peer: DEFAULT_MAX_COUNT_CONCURRENT_REQUESTS_PER_PEER,
            soft_limit_byte_size_pooled_transactions_response:
                SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESPONSE,
            soft_limit_byte_size_pooled_transactions_response_on_pack_request: DEFAULT_SOFT_LIMIT_BYTE_SIZE_POOLED_TRANSACTIONS_RESP_ON_PACK_GET_POOLED_TRANSACTIONS_REQ,
            max_pending_pool_imports: DEFAULT_MAX_COUNT_PENDING_POOL_IMPORTS,
            max_seen_tx_history: DEFAULT_MAX_COUNT_TRANSACTIONS_SEEN_BY_PEER,
            max_capacity_cache_txns_pending_fetch: DEFAULT_MAX_CAPACITY_CACHE_PENDING_FETCH,
            net_if: None,
            tx_propagation_policy: TransactionPropagationKind::default(),
            tx_ingress_policy: TransactionIngressPolicy::default(),
            disable_tx_gossip: false,
            propagation_mode: TransactionPropagationMode::Sqrt,
            required_block_hashes: vec![],
            network_id: None,
            netrestrict: None,
            enforce_enr_fork_id: false,
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
    pub fn apply_to_builder<N>(
        &self,
        mut network_config_builder: NetworkConfigBuilder<N>,
        rlpx_tcp_socket: SocketAddr,
        boot_nodes: impl IntoIterator<Item = NodeRecord>,
    ) -> NetworkConfigBuilder<N>
    where
        N: NetworkPrimitives,
    {
        if self.disable_discovery || self.disable_dns_discovery {
            network_config_builder = network_config_builder.disable_dns_discovery();
        }

        if self.disable_discovery || self.disable_discv4_discovery {
            network_config_builder = network_config_builder.disable_discv4_discovery();
        }

        if self.disable_nat {
            // we only check for `disable-nat` here and not for disable discovery because nat:extip can be used without discovery: <https://github.com/paradigmxyz/reth/issues/14878>
            network_config_builder = network_config_builder.disable_nat();
        }

        if self.should_enable_discv5() {
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

    /// Returns true if discv5 discovery should be configured
    const fn should_enable_discv5(&self) -> bool {
        if self.disable_discovery {
            return false;
        }

        self.enable_discv5_discovery ||
            self.discv5_addr.is_some() ||
            self.discv5_addr_ipv6.is_some()
    }

    /// Set the discovery port to zero, to allow the OS to assign a random unused port when
    /// discovery binds to the socket.
    pub const fn with_unused_discovery_port(mut self) -> Self {
        self.port = 0;
        self
    }

    /// Set the discovery V5 port
    pub const fn with_discv5_port(mut self, port: u16) -> Self {
        self.discv5_port = port;
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

/// Parse a block number=hash pair or just a hash into `BlockNumHash`
fn parse_block_num_hash(s: &str) -> Result<BlockNumHash, String> {
    if let Some((num_str, hash_str)) = s.split_once('=') {
        let number = num_str.parse().map_err(|_| format!("Invalid block number: {}", num_str))?;
        let hash = B256::from_str(hash_str).map_err(|_| format!("Invalid hash: {}", hash_str))?;
        Ok(BlockNumHash::new(number, hash))
    } else {
        // For backward compatibility, treat as hash-only with number 0
        let hash = B256::from_str(s).map_err(|_| format!("Invalid hash: {}", s))?;
        Ok(BlockNumHash::new(0, hash))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use reth_chainspec::MAINNET;
    use reth_config::Config;
    use reth_network_peers::NodeRecord;
    use secp256k1::SecretKey;
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

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
            let retries_str = retries.to_string();
            let args = CommandParser::<NetworkArgs>::parse_from([
                "reth",
                "--dns-retries",
                retries_str.as_str(),
            ])
            .args;

            assert_eq!(args.dns_retries, retries);
        }
    }

    #[test]
    fn parse_disable_tx_gossip_args() {
        let args = CommandParser::<NetworkArgs>::parse_from(["reth", "--disable-tx-gossip"]).args;
        assert!(args.disable_tx_gossip);
    }

    #[test]
    fn parse_max_peers_flag() {
        let args = CommandParser::<NetworkArgs>::parse_from(["reth", "--max-peers", "90"]).args;

        assert_eq!(args.max_peers, Some(90));
        assert_eq!(args.max_outbound_peers, None);
        assert_eq!(args.max_inbound_peers, None);
        assert_eq!(args.resolved_max_outbound_peers(), Some(30));
        assert_eq!(args.resolved_max_inbound_peers(), Some(60));
    }

    #[test]
    fn max_peers_conflicts_with_outbound() {
        let result = CommandParser::<NetworkArgs>::try_parse_from([
            "reth",
            "--max-peers",
            "90",
            "--max-outbound-peers",
            "50",
        ]);
        assert!(
            result.is_err(),
            "Should fail when both --max-peers and --max-outbound-peers are used"
        );
    }

    #[test]
    fn max_peers_conflicts_with_inbound() {
        let result = CommandParser::<NetworkArgs>::try_parse_from([
            "reth",
            "--max-peers",
            "90",
            "--max-inbound-peers",
            "30",
        ]);
        assert!(
            result.is_err(),
            "Should fail when both --max-peers and --max-inbound-peers are used"
        );
    }

    #[test]
    fn max_peers_split_calculation() {
        let args = CommandParser::<NetworkArgs>::parse_from(["reth", "--max-peers", "90"]).args;

        assert_eq!(args.max_peers, Some(90));
        assert_eq!(args.resolved_max_outbound_peers(), Some(30));
        assert_eq!(args.resolved_max_inbound_peers(), Some(60));
    }

    #[test]
    fn max_peers_small_values() {
        let args1 = CommandParser::<NetworkArgs>::parse_from(["reth", "--max-peers", "1"]).args;
        assert_eq!(args1.resolved_max_outbound_peers(), Some(1));
        assert_eq!(args1.resolved_max_inbound_peers(), Some(0));

        let args2 = CommandParser::<NetworkArgs>::parse_from(["reth", "--max-peers", "2"]).args;
        assert_eq!(args2.resolved_max_outbound_peers(), Some(1));
        assert_eq!(args2.resolved_max_inbound_peers(), Some(1));

        let args3 = CommandParser::<NetworkArgs>::parse_from(["reth", "--max-peers", "3"]).args;
        assert_eq!(args3.resolved_max_outbound_peers(), Some(1));
        assert_eq!(args3.resolved_max_inbound_peers(), Some(2));
    }

    #[test]
    fn resolved_peers_without_max_peers() {
        let args = CommandParser::<NetworkArgs>::parse_from([
            "reth",
            "--max-outbound-peers",
            "75",
            "--max-inbound-peers",
            "15",
        ])
        .args;

        assert_eq!(args.max_peers, None);
        assert_eq!(args.resolved_max_outbound_peers(), Some(75));
        assert_eq!(args.resolved_max_inbound_peers(), Some(15));
    }

    #[test]
    fn resolved_peers_with_defaults() {
        let args = CommandParser::<NetworkArgs>::parse_from(["reth"]).args;

        assert_eq!(args.max_peers, None);
        assert_eq!(args.resolved_max_outbound_peers(), None);
        assert_eq!(args.resolved_max_inbound_peers(), None);
    }

    #[test]
    fn network_args_default_sanity_test() {
        let default_args = NetworkArgs::default();
        let args = CommandParser::<NetworkArgs>::parse_from(["reth"]).args;

        assert_eq!(args, default_args);
    }

    #[test]
    fn parse_required_block_hashes() {
        let args = CommandParser::<NetworkArgs>::parse_from([
            "reth",
            "--required-block-hashes",
            "0x1111111111111111111111111111111111111111111111111111111111111111,23115201=0x2222222222222222222222222222222222222222222222222222222222222222",
        ])
        .args;

        assert_eq!(args.required_block_hashes.len(), 2);
        // First hash without block number (should default to 0)
        assert_eq!(args.required_block_hashes[0].number, 0);
        assert_eq!(
            args.required_block_hashes[0].hash.to_string(),
            "0x1111111111111111111111111111111111111111111111111111111111111111"
        );
        // Second with block number=hash format
        assert_eq!(args.required_block_hashes[1].number, 23115201);
        assert_eq!(
            args.required_block_hashes[1].hash.to_string(),
            "0x2222222222222222222222222222222222222222222222222222222222222222"
        );
    }

    #[test]
    fn parse_empty_required_block_hashes() {
        let args = CommandParser::<NetworkArgs>::parse_from(["reth"]).args;
        assert!(args.required_block_hashes.is_empty());
    }

    #[test]
    fn test_parse_block_num_hash() {
        // Test hash only format
        let result = parse_block_num_hash(
            "0x1111111111111111111111111111111111111111111111111111111111111111",
        );
        assert!(result.is_ok());
        assert_eq!(result.unwrap().number, 0);

        // Test block_number=hash format
        let result = parse_block_num_hash(
            "23115201=0x2222222222222222222222222222222222222222222222222222222222222222",
        );
        assert!(result.is_ok());
        assert_eq!(result.unwrap().number, 23115201);

        // Test invalid formats
        assert!(parse_block_num_hash("invalid").is_err());
        assert!(parse_block_num_hash(
            "abc=0x1111111111111111111111111111111111111111111111111111111111111111"
        )
        .is_err());
    }

    #[test]
    fn parse_p2p_secret_key_hex() {
        let hex = "4c0883a69102937d6231471b5dbb6204fe512961708279f8c5c58b3b9c4e8b8f";
        let args =
            CommandParser::<NetworkArgs>::parse_from(["reth", "--p2p-secret-key-hex", hex]).args;

        let expected: B256 = hex.parse().unwrap();
        assert_eq!(args.p2p_secret_key_hex, Some(expected));
        assert_eq!(args.p2p_secret_key, None);
    }

    #[test]
    fn parse_p2p_secret_key_hex_with_0x_prefix() {
        let hex = "0x4c0883a69102937d6231471b5dbb6204fe512961708279f8c5c58b3b9c4e8b8f";
        let args =
            CommandParser::<NetworkArgs>::parse_from(["reth", "--p2p-secret-key-hex", hex]).args;

        let expected: B256 = hex.parse().unwrap();
        assert_eq!(args.p2p_secret_key_hex, Some(expected));
        assert_eq!(args.p2p_secret_key, None);
    }

    #[test]
    fn test_p2p_secret_key_and_hex_are_mutually_exclusive() {
        let result = CommandParser::<NetworkArgs>::try_parse_from([
            "reth",
            "--p2p-secret-key",
            "/path/to/key",
            "--p2p-secret-key-hex",
            "4c0883a69102937d6231471b5dbb6204fe512961708279f8c5c58b3b9c4e8b8f",
        ]);

        assert!(result.is_err());
    }

    #[test]
    fn test_secret_key_method_with_hex() {
        let hex = "4c0883a69102937d6231471b5dbb6204fe512961708279f8c5c58b3b9c4e8b8f";
        let args =
            CommandParser::<NetworkArgs>::parse_from(["reth", "--p2p-secret-key-hex", hex]).args;

        let temp_dir = std::env::temp_dir();
        let default_path = temp_dir.join("default_key");
        let secret_key = args.secret_key(default_path).unwrap();

        // Verify the secret key matches the hex input
        assert_eq!(alloy_primitives::hex::encode(secret_key.secret_bytes()), hex);
    }

    #[test]
    fn parse_netrestrict_single_network() {
        let args =
            CommandParser::<NetworkArgs>::parse_from(["reth", "--netrestrict", "192.168.0.0/16"])
                .args;

        assert_eq!(args.netrestrict, Some("192.168.0.0/16".to_string()));

        let ip_filter = args.ip_filter().unwrap();
        assert!(ip_filter.has_restrictions());
        assert!(ip_filter.is_allowed(&"192.168.1.1".parse().unwrap()));
        assert!(!ip_filter.is_allowed(&"10.0.0.1".parse().unwrap()));
    }

    #[test]
    fn parse_netrestrict_multiple_networks() {
        let args = CommandParser::<NetworkArgs>::parse_from([
            "reth",
            "--netrestrict",
            "192.168.0.0/16,10.0.0.0/8",
        ])
        .args;

        assert_eq!(args.netrestrict, Some("192.168.0.0/16,10.0.0.0/8".to_string()));

        let ip_filter = args.ip_filter().unwrap();
        assert!(ip_filter.has_restrictions());
        assert!(ip_filter.is_allowed(&"192.168.1.1".parse().unwrap()));
        assert!(ip_filter.is_allowed(&"10.5.10.20".parse().unwrap()));
        assert!(!ip_filter.is_allowed(&"172.16.0.1".parse().unwrap()));
    }

    #[test]
    fn parse_netrestrict_ipv6() {
        let args =
            CommandParser::<NetworkArgs>::parse_from(["reth", "--netrestrict", "2001:db8::/32"])
                .args;

        let ip_filter = args.ip_filter().unwrap();
        assert!(ip_filter.has_restrictions());
        assert!(ip_filter.is_allowed(&"2001:db8::1".parse().unwrap()));
        assert!(!ip_filter.is_allowed(&"2001:db9::1".parse().unwrap()));
    }

    #[test]
    fn netrestrict_not_set() {
        let args = CommandParser::<NetworkArgs>::parse_from(["reth"]).args;
        assert_eq!(args.netrestrict, None);

        let ip_filter = args.ip_filter().unwrap();
        assert!(!ip_filter.has_restrictions());
        assert!(ip_filter.is_allowed(&"192.168.1.1".parse().unwrap()));
        assert!(ip_filter.is_allowed(&"10.0.0.1".parse().unwrap()));
    }

    #[test]
    fn netrestrict_invalid_cidr() {
        let args =
            CommandParser::<NetworkArgs>::parse_from(["reth", "--netrestrict", "invalid-cidr"])
                .args;

        assert!(args.ip_filter().is_err());
    }

    #[test]
    fn network_config_preserves_basic_nodes_from_peers_file() {
        let enode = "enode://6f8a80d14311c39f35f516fa664deaaaa13e85b2f7493f37f6144d86991ec012937307647bd3b9a82abe2974e1407241d54947bbb39763a4cac9f77166ad92a0@10.3.58.6:30303?discport=30301";
        let unique = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();

        let peers_file = std::env::temp_dir().join(format!("reth_peers_test_{}.json", unique));
        fs::write(&peers_file, format!("[\"{}\"]", enode)).expect("write peers file");

        // Build NetworkArgs with peers_file set and no_persist_peers=false
        let args = NetworkArgs {
            peers_file: Some(peers_file.clone()),
            no_persist_peers: false,
            ..Default::default()
        };

        // Build the network config using a deterministic secret key
        let secret_key = SecretKey::from_byte_array(&[1u8; 32]).unwrap();
        let builder = args.network_config::<reth_network::EthNetworkPrimitives>(
            &Config::default(),
            MAINNET.clone(),
            secret_key,
            peers_file.clone(),
            Runtime::test(),
        );

        let net_cfg = builder.build_with_noop_provider(MAINNET.clone());

        // Assert persisted_peers contains our node (legacy format is auto-converted)
        let node: NodeRecord = enode.parse().unwrap();
        assert!(net_cfg.peers_config.persisted_peers.iter().any(|p| p.record == node));

        // Cleanup
        let _ = fs::remove_file(&peers_file);
    }
}

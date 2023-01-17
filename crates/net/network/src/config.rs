//! Network config support

use crate::{
    error::NetworkError,
    import::{BlockImport, ProofOfStakeBlockImport},
    peers::PeersConfig,
    session::SessionsConfig,
    NetworkHandle, NetworkManager,
};
use reth_discv4::{Discv4Config, Discv4ConfigBuilder, DEFAULT_DISCOVERY_PORT};
use reth_primitives::{Chain, ForkFilter, Hardfork, NodeRecord, PeerId, H256, MAINNET_GENESIS};
use reth_provider::{BlockProvider, HeaderProvider};
use reth_tasks::TaskExecutor;
use secp256k1::{SecretKey, SECP256K1};
use std::{
    collections::HashSet,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    sync::Arc,
};

/// reexports for convenience
#[doc(hidden)]
mod __reexport {
    pub use reth_discv4::bootnodes::*;
    pub use secp256k1::SecretKey;
}
pub use __reexport::*;
use reth_dns_discovery::DnsDiscoveryConfig;
use reth_ecies::util::pk2id;
use reth_eth_wire::{HelloMessage, Status};

/// Convenience function to create a new random [`SecretKey`]
pub fn rng_secret_key() -> SecretKey {
    SecretKey::new(&mut rand::thread_rng())
}

/// All network related initialization settings.
pub struct NetworkConfig<C> {
    /// The client type that can interact with the chain.
    pub client: Arc<C>,
    /// The node's secret key, from which the node's identity is derived.
    pub secret_key: SecretKey,
    /// All boot nodes to start network discovery with.
    pub boot_nodes: HashSet<NodeRecord>,
    /// How to set up discovery over DNS.
    pub dns_discovery_config: Option<DnsDiscoveryConfig>,
    /// How to set up discovery.
    pub discovery_v4_config: Option<Discv4Config>,
    /// Address to use for discovery
    pub discovery_addr: SocketAddr,
    /// Address to listen for incoming connections
    pub listener_addr: SocketAddr,
    /// How to instantiate peer manager.
    pub peers_config: PeersConfig,
    /// How to configure the [SessionManager](crate::session::SessionManager).
    pub sessions_config: SessionsConfig,
    /// The id of the network
    pub chain: Chain,
    /// Genesis hash of the network
    pub genesis_hash: H256,
    /// The [`ForkFilter`] to use at launch for authenticating sessions.
    ///
    /// See also <https://github.com/ethereum/EIPs/blob/master/EIPS/eip-2124.md#stale-software-examples>
    ///
    /// For sync from block `0`, this should be the default chain [`ForkFilter`] beginning at the
    /// first hardfork, `Frontier` for mainnet.
    pub fork_filter: ForkFilter,
    /// The block importer type.
    pub block_import: Box<dyn BlockImport>,
    /// The default mode of the network.
    pub network_mode: NetworkMode,
    /// The executor to use for spawning tasks.
    pub executor: Option<TaskExecutor>,
    /// The `Status` message to send to peers at the beginning.
    pub status: Status,
    /// Sets the hello message for the p2p handshake in RLPx
    pub hello_message: HelloMessage,
}

// === impl NetworkConfig ===

impl<C> NetworkConfig<C> {
    /// Create a new instance with all mandatory fields set, rest is field with defaults.
    pub fn new(client: Arc<C>, secret_key: SecretKey) -> Self {
        Self::builder(client, secret_key).build()
    }

    /// Convenience method for creating the corresponding builder type
    pub fn builder(client: Arc<C>, secret_key: SecretKey) -> NetworkConfigBuilder<C> {
        NetworkConfigBuilder::new(client, secret_key)
    }

    /// Sets the config to use for the discovery v4 protocol.
    pub fn set_discovery_v4(mut self, discovery_config: Discv4Config) -> Self {
        self.discovery_v4_config = Some(discovery_config);
        self
    }

    /// Sets the address for the incoming connection listener.
    pub fn set_listener_addr(mut self, listener_addr: SocketAddr) -> Self {
        self.listener_addr = listener_addr;
        self
    }
}

impl<C> NetworkConfig<C>
where
    C: BlockProvider + HeaderProvider + 'static,
{
    /// Starts the networking stack given a [NetworkConfig] and returns a handle to the network.
    pub async fn start_network(self) -> Result<NetworkHandle, NetworkError> {
        let client = self.client.clone();
        let (handle, network, _txpool, eth) =
            NetworkManager::builder(self).await?.request_handler(client).split_with_handle();

        tokio::task::spawn(network);
        // TODO: tokio::task::spawn(txpool);
        tokio::task::spawn(eth);
        Ok(handle)
    }
}

/// Builder for [`NetworkConfig`](struct.NetworkConfig.html).
#[allow(missing_docs)]
pub struct NetworkConfigBuilder<C> {
    /// The client type that can interact with the chain.
    client: Arc<C>,
    /// The node's secret key, from which the node's identity is derived.
    secret_key: SecretKey,
    /// How to configure discovery over DNS.
    dns_discovery_config: Option<DnsDiscoveryConfig>,
    /// How to set up discovery.
    discovery_v4_builder: Option<Discv4ConfigBuilder>,
    /// All boot nodes to start network discovery with.
    boot_nodes: HashSet<NodeRecord>,
    /// Address to use for discovery
    discovery_addr: Option<SocketAddr>,
    /// Listener for incoming connections
    listener_addr: Option<SocketAddr>,
    /// How to instantiate peer manager.
    peers_config: Option<PeersConfig>,
    /// How to configure the sessions manager
    sessions_config: Option<SessionsConfig>,
    /// The network's chain id
    chain: Chain,
    /// Network genesis hash
    genesis_hash: H256,
    /// The block importer type.
    block_import: Box<dyn BlockImport>,
    /// The default mode of the network.
    network_mode: NetworkMode,
    /// The executor to use for spawning tasks.
    executor: Option<TaskExecutor>,
    /// The `Status` message to send to peers at the beginning.
    status: Option<Status>,
    /// Sets the hello message for the p2p handshake in RLPx
    hello_message: Option<HelloMessage>,
    /// The [`ForkFilter`] to use at launch for authenticating sessions.
    fork_filter: Option<ForkFilter>,
    /// Head used to start set for the fork filter
    head: Option<u64>,
}

// === impl NetworkConfigBuilder ===

#[allow(missing_docs)]
impl<C> NetworkConfigBuilder<C> {
    pub fn new(client: Arc<C>, secret_key: SecretKey) -> Self {
        Self {
            client,
            secret_key,
            dns_discovery_config: Some(Default::default()),
            discovery_v4_builder: Some(Default::default()),
            boot_nodes: Default::default(),
            discovery_addr: None,
            listener_addr: None,
            peers_config: None,
            sessions_config: None,
            chain: Chain::Named(reth_primitives::rpc::Chain::Mainnet),
            genesis_hash: MAINNET_GENESIS,
            block_import: Box::<ProofOfStakeBlockImport>::default(),
            network_mode: Default::default(),
            executor: None,
            status: None,
            hello_message: None,
            fork_filter: None,
            head: None,
        }
    }

    /// Returns the configured [`PeerId`]
    pub fn get_peer_id(&self) -> PeerId {
        pk2id(&self.secret_key.public_key(SECP256K1))
    }

    /// Sets the `Status` message to send when connecting to peers.
    ///
    /// ```
    /// # use reth_eth_wire::Status;
    /// # use reth_network::NetworkConfigBuilder;
    /// # fn builder<C>(builder: NetworkConfigBuilder<C>) {
    ///     builder.status(
    ///         Status::builder().build()
    /// );
    /// # }
    /// ```
    pub fn status(mut self, status: Status) -> Self {
        self.status = Some(status);
        self
    }

    /// Sets the chain ID.
    pub fn chain_id<Id: Into<Chain>>(mut self, chain_id: Id) -> Self {
        self.chain = chain_id.into();
        self
    }

    /// Sets the `HelloMessage` to send when connecting to peers.
    ///
    /// ```
    /// # use reth_eth_wire::HelloMessage;
    /// # use reth_network::NetworkConfigBuilder;
    /// # fn builder<C>(builder: NetworkConfigBuilder<C>) {
    ///    let peer_id = builder.get_peer_id();
    ///     builder.hello_message(
    ///         HelloMessage::builder(peer_id).build()
    /// );
    /// # }
    /// ```
    pub fn hello_message(mut self, hello_message: HelloMessage) -> Self {
        self.hello_message = Some(hello_message);
        self
    }

    /// Set a custom peer config for how peers are handled
    pub fn peer_config(mut self, config: PeersConfig) -> Self {
        self.peers_config = Some(config);
        self
    }

    /// Sets the executor to use for spawning tasks.
    pub fn executor(mut self, executor: TaskExecutor) -> Self {
        self.executor = Some(executor);
        self
    }

    /// Sets a custom config for how sessions are handled.
    pub fn sessions_config(mut self, config: SessionsConfig) -> Self {
        self.sessions_config = Some(config);
        self
    }

    /// Sets the genesis hash for the network.
    pub fn genesis_hash(mut self, genesis_hash: H256) -> Self {
        self.genesis_hash = genesis_hash;
        self
    }

    /// Sets the [`BlockImport`] type to configure.
    pub fn block_import<T: BlockImport + 'static>(mut self, block_import: T) -> Self {
        self.block_import = Box::new(block_import);
        self
    }

    /// Sets the socket address the network will listen on
    pub fn listener_addr(mut self, listener_addr: SocketAddr) -> Self {
        self.listener_addr = Some(listener_addr);
        self
    }

    /// Sets the socket address the discovery network will listen on
    pub fn discovery_addr(mut self, discovery_addr: SocketAddr) -> Self {
        self.discovery_addr = Some(discovery_addr);
        self
    }

    /// Sets the discv4 config to use.
    pub fn discovery(mut self, builder: Discv4ConfigBuilder) -> Self {
        self.discovery_v4_builder = Some(builder);
        self
    }

    /// Sets the dns discovery config to use.
    pub fn dns_discovery(mut self, config: DnsDiscoveryConfig) -> Self {
        self.dns_discovery_config = Some(config);
        self
    }

    /// Sets the boot nodes.
    pub fn boot_nodes(mut self, nodes: impl IntoIterator<Item = NodeRecord>) -> Self {
        self.boot_nodes = nodes.into_iter().collect();
        self
    }

    /// Sets the discovery service off on true.
    pub fn set_discovery(mut self, disable_discovery: bool) -> Self {
        if disable_discovery {
            self.disable_discovery();
        }
        self
    }

    /// disables discovery.
    pub fn disable_discovery(&mut self) {
        self.discovery_v4_builder = None;
    }

    /// Consumes the type and creates the actual [`NetworkConfig`]
    pub fn build(self) -> NetworkConfig<C> {
        let peer_id = self.get_peer_id();
        let Self {
            client,
            secret_key,
            mut dns_discovery_config,
            discovery_v4_builder,
            boot_nodes,
            discovery_addr,
            listener_addr,
            peers_config,
            sessions_config,
            chain,
            genesis_hash,
            block_import,
            network_mode,
            executor,
            status,
            hello_message,
            fork_filter,
            head,
        } = self;

        let listener_addr = listener_addr.unwrap_or_else(|| {
            SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, DEFAULT_DISCOVERY_PORT))
        });

        let mut hello_message =
            hello_message.unwrap_or_else(|| HelloMessage::builder(peer_id).build());
        hello_message.port = listener_addr.port();

        // get the fork filter
        let fork_filter = fork_filter.unwrap_or_else(|| {
            let head = head.unwrap_or_default();
            // TODO(mattsse): this should be chain agnostic: <https://github.com/paradigmxyz/reth/issues/485>
            ForkFilter::new(head, genesis_hash, Hardfork::all_forks())
        });

        // If default DNS config is used then we add the known dns network to bootstrap from
        if let Some(dns_networks) =
            dns_discovery_config.as_mut().and_then(|c| c.bootstrap_dns_networks.as_mut())
        {
            if dns_networks.is_empty() {
                if let Some(link) = chain.public_dns_network_protocol() {
                    dns_networks.insert(link.parse().expect("is valid DNS link entry"));
                }
            }
        }

        NetworkConfig {
            client,
            secret_key,
            boot_nodes,
            dns_discovery_config,
            discovery_v4_config: discovery_v4_builder.map(|builder| builder.build()),
            discovery_addr: discovery_addr.unwrap_or_else(|| {
                SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, DEFAULT_DISCOVERY_PORT))
            }),
            listener_addr,
            peers_config: peers_config.unwrap_or_default(),
            sessions_config: sessions_config.unwrap_or_default(),
            chain,
            genesis_hash,
            block_import,
            network_mode,
            executor,
            status: status.unwrap_or_default(),
            hello_message,
            fork_filter,
        }
    }
}

/// Describes the mode of the network wrt. POS or POW.
///
/// This affects block propagation in the `eth` sub-protocol [EIP-3675](https://eips.ethereum.org/EIPS/eip-3675#devp2p)
///
/// In POS `NewBlockHashes` and `NewBlock` messages become invalid.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Default)]
pub enum NetworkMode {
    /// Network is in proof-of-work mode.
    Work,
    /// Network is in proof-of-stake mode
    #[default]
    Stake,
}

// === impl NetworkMode ===

impl NetworkMode {
    /// Returns true if network has entered proof-of-stake
    pub fn is_stake(&self) -> bool {
        matches!(self, NetworkMode::Stake)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::thread_rng;
    use reth_dns_discovery::tree::LinkEntry;
    use reth_provider::test_utils::NoopProvider;

    fn builder() -> NetworkConfigBuilder<NoopProvider> {
        let secret_key = SecretKey::new(&mut thread_rng());
        NetworkConfig::builder(Arc::new(NoopProvider::default()), secret_key)
    }

    #[test]
    fn test_network_dns_defaults() {
        let config = builder().build();

        let dns = config.dns_discovery_config.unwrap();
        let bootstrap_nodes = dns.bootstrap_dns_networks.unwrap();
        let mainnet_dns: LinkEntry =
            Chain::mainnet().public_dns_network_protocol().unwrap().parse().unwrap();
        assert!(bootstrap_nodes.contains(&mainnet_dns));
        assert_eq!(bootstrap_nodes.len(), 1);
    }
}

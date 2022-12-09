use crate::{
    import::{BlockImport, ProofOfStakeBlockImport},
    peers::PeersConfig,
    session::SessionsConfig,
};
use reth_discv4::{Discv4Config, Discv4ConfigBuilder, NodeRecord, DEFAULT_DISCOVERY_PORT};
use reth_primitives::{Chain, ForkId, H256};
use secp256k1::SecretKey;
use std::{
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    sync::Arc,
};

/// All network related initialization settings.
pub struct NetworkConfig<C> {
    /// The client type that can interact with the chain.
    pub client: Arc<C>,
    /// The node's secret key, from which the node's identity is derived.
    pub secret_key: SecretKey,
    /// All boot nodes to start network discovery with.
    pub boot_nodes: Vec<NodeRecord>,
    /// How to set up discovery.
    pub discovery_v4_config: Discv4Config,
    /// Address to use for discovery
    pub discovery_addr: SocketAddr,
    /// Address to listen for incoming connections
    pub listener_addr: SocketAddr,
    /// How to instantiate peer manager.
    pub peers_config: PeersConfig,
    /// How to configure the [`SessionManager`]
    pub sessions_config: SessionsConfig,
    /// A fork identifier as defined by EIP-2124.
    /// Serves as the chain compatibility identifier.
    pub fork_id: Option<ForkId>,
    /// The id of the network
    pub chain: Chain,
    /// Genesis hash of the network
    pub genesis_hash: H256,
    /// The block importer type.
    pub block_import: Box<dyn BlockImport>,
    /// The default mode of the network.
    pub network_mode: NetworkMode,
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
        self.discovery_v4_config = discovery_config;
        self
    }

    /// Sets the address for the incoming connection listener.
    pub fn set_listener_addr(mut self, listener_addr: SocketAddr) -> Self {
        self.listener_addr = listener_addr;
        self
    }
}

/// Builder for [`NetworkConfig`](struct.NetworkConfig.html).
#[allow(missing_docs)]
pub struct NetworkConfigBuilder<C> {
    /// The client type that can interact with the chain.
    client: Arc<C>,
    /// The node's secret key, from which the node's identity is derived.
    secret_key: SecretKey,
    /// How to set up discovery.
    discovery_v4_builder: Discv4ConfigBuilder,
    /// All boot nodes to start network discovery with.
    boot_nodes: Vec<NodeRecord>,
    /// Address to use for discovery
    discovery_addr: Option<SocketAddr>,
    /// Listener for incoming connections
    listener_addr: Option<SocketAddr>,
    /// How to instantiate peer manager.
    peers_config: Option<PeersConfig>,
    /// How to configure the sessions manager
    sessions_config: Option<SessionsConfig>,
    /// A fork identifier as defined by EIP-2124.
    /// Serves as the chain compatibility identifier.
    fork_id: Option<ForkId>,
    /// The network's chain id
    chain: Chain,
    /// Network genesis hash
    genesis_hash: H256,
    /// The block importer type.
    block_import: Box<dyn BlockImport>,
    /// The default mode of the network.
    network_mode: NetworkMode,
}

// === impl NetworkConfigBuilder ===

#[allow(missing_docs)]
impl<C> NetworkConfigBuilder<C> {
    pub fn new(client: Arc<C>, secret_key: SecretKey) -> Self {
        Self {
            client,
            secret_key,
            discovery_v4_builder: Default::default(),
            boot_nodes: vec![],
            discovery_addr: None,
            listener_addr: None,
            peers_config: None,
            sessions_config: None,
            fork_id: None,
            chain: Chain::Named(reth_primitives::rpc::Chain::Mainnet),
            genesis_hash: Default::default(),
            block_import: Box::<ProofOfStakeBlockImport>::default(),
            network_mode: Default::default(),
        }
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
        self.discovery_v4_builder = builder;
        self
    }

    /// Sets the discv4 config to use.
    pub fn boot_nodes(mut self, nodes: impl IntoIterator<Item = NodeRecord>) -> Self {
        self.boot_nodes = nodes.into_iter().collect();
        self
    }

    /// Consumes the type and creates the actual [`NetworkConfig`]
    pub fn build(self) -> NetworkConfig<C> {
        let Self {
            client,
            secret_key,
            discovery_v4_builder,
            boot_nodes,
            discovery_addr,
            listener_addr,
            peers_config,
            sessions_config,
            fork_id,
            chain,
            genesis_hash,
            block_import,
            network_mode,
        } = self;
        NetworkConfig {
            client,
            secret_key,
            boot_nodes,
            discovery_v4_config: discovery_v4_builder.build(),
            discovery_addr: discovery_addr.unwrap_or_else(|| {
                SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, DEFAULT_DISCOVERY_PORT))
            }),
            listener_addr: listener_addr.unwrap_or_else(|| {
                SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, DEFAULT_DISCOVERY_PORT))
            }),
            peers_config: peers_config.unwrap_or_default(),
            sessions_config: sessions_config.unwrap_or_default(),
            fork_id,
            chain,
            genesis_hash,
            block_import,
            network_mode,
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

use crate::{peers::PeersConfig, session::SessionsConfig, sync::StateSync};
use reth_discv4::{Discv4Config, Discv4ConfigBuilder, DEFAULT_DISCOVERY_PORT};
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
    /// How to set up discovery.
    pub discovery_v4_config: Discv4Config,
    /// Address to use for discovery
    pub discovery_addr: SocketAddr,
    /// Address to listen for incoming connections
    pub listener_addr: SocketAddr,
    /// The sync instance to use
    pub state_sync: Box<dyn StateSync>,
    /// How to instantiate peer manager.
    pub peers_config: PeersConfig,
    /// How to configure the [`SessionManager`]
    pub sessions_config: SessionsConfig,
}

// === impl NetworkConfig ===

impl<C> NetworkConfig<C> {
    /// Create a new instance with all mandatory fields set, rest is field with defaults.
    pub fn new(client: Arc<C>, secret_key: SecretKey, state_sync: Box<dyn StateSync>) -> Self {
        Self::builder(client, secret_key, state_sync).build()
    }

    /// Convenience method for creating the corresponding builder type
    pub fn builder(
        client: Arc<C>,
        secret_key: SecretKey,
        state_sync: Box<dyn StateSync>,
    ) -> NetworkConfigBuilder<C> {
        NetworkConfigBuilder::new(client, secret_key, state_sync)
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
pub struct NetworkConfigBuilder<C> {
    /// The client type that can interact with the chain.
    client: Arc<C>,
    /// The node's secret key, from which the node's identity is derived.
    secret_key: SecretKey,
    /// How to set up discovery.
    discovery_v4_builder: Discv4ConfigBuilder,
    /// Address to use for discovery
    discovery_addr: Option<SocketAddr>,
    /// Listener for incoming connections
    listener_addr: Option<SocketAddr>,
    /// Sync type to use
    state_sync: Box<dyn StateSync>,
    /// How to instantiate peer manager.
    peers_config: Option<PeersConfig>,
    /// How to configure the sessions manager
    sessions_config: Option<SessionsConfig>,
}

// === impl NetworkConfigBuilder ===

#[allow(missing_docs)]
impl<C> NetworkConfigBuilder<C> {
    pub fn new(client: Arc<C>, secret_key: SecretKey, state_sync: Box<dyn StateSync>) -> Self {
        Self {
            client,
            secret_key,
            discovery_v4_builder: Default::default(),
            discovery_addr: None,
            listener_addr: None,
            state_sync,
            peers_config: None,
            sessions_config: None,
        }
    }

    /// Consumes the type and creates the actual [`NetworkConfig`]
    pub fn build(self) -> NetworkConfig<C> {
        let Self {
            client,
            secret_key,
            discovery_v4_builder,
            discovery_addr,
            listener_addr,
            state_sync,
            peers_config,
            sessions_config,
        } = self;
        NetworkConfig {
            client,
            secret_key,
            discovery_v4_config: discovery_v4_builder.build(),
            discovery_addr: discovery_addr.unwrap_or_else(|| {
                SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, DEFAULT_DISCOVERY_PORT))
            }),
            listener_addr: listener_addr.unwrap_or_else(|| {
                SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, DEFAULT_DISCOVERY_PORT))
            }),
            state_sync,
            peers_config: peers_config.unwrap_or_default(),
            sessions_config: sessions_config.unwrap_or_default(),
        }
    }
}

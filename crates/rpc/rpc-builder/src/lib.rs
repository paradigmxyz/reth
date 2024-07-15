//! Configure reth RPC.
//!
//! This crate contains several builder and config types that allow to configure the selection of
//! [`RethRpcModule`] specific to transports (ws, http, ipc).
//!
//! The [`RpcModuleBuilder`] is the main entrypoint for configuring all reth modules. It takes
//! instances of components required to start the servers, such as provider impls, network and
//! transaction pool. [`RpcModuleBuilder::build`] returns a [`TransportRpcModules`] which contains
//! the transport specific config (what APIs are available via this transport).
//!
//! The [`RpcServerConfig`] is used to configure the [`RpcServer`] type which contains all transport
//! implementations (http server, ws server, ipc server). [`RpcServer::start`] requires the
//! [`TransportRpcModules`] so it can start the servers with the configured modules.
//!
//! # Examples
//!
//! Configure only an http server with a selection of [`RethRpcModule`]s
//!
//! ```
//! use reth_evm::ConfigureEvm;
//! use reth_network_api::{NetworkInfo, Peers};
//! use reth_provider::{
//!     AccountReader, BlockReaderIdExt, CanonStateSubscriptions, ChainSpecProvider,
//!     ChangeSetReader, EvmEnvProvider, StateProviderFactory,
//! };
//! use reth_rpc_builder::{
//!     RethRpcModule, RpcModuleBuilder, RpcServerConfig, ServerBuilder, TransportRpcModuleConfig,
//! };
//! use reth_tasks::TokioTaskExecutor;
//! use reth_transaction_pool::TransactionPool;
//! pub async fn launch<Provider, Pool, Network, Events, EvmConfig>(
//!     provider: Provider,
//!     pool: Pool,
//!     network: Network,
//!     events: Events,
//!     evm_config: EvmConfig,
//! ) where
//!     Provider: AccountReader
//!         + BlockReaderIdExt
//!         + ChainSpecProvider
//!         + ChangeSetReader
//!         + StateProviderFactory
//!         + EvmEnvProvider
//!         + Clone
//!         + Unpin
//!         + 'static,
//!     Pool: TransactionPool + Clone + 'static,
//!     Network: NetworkInfo + Peers + Clone + 'static,
//!     Events: CanonStateSubscriptions + Clone + 'static,
//!     EvmConfig: ConfigureEvm + 'static,
//! {
//!     // configure the rpc module per transport
//!     let transports = TransportRpcModuleConfig::default().with_http(vec![
//!         RethRpcModule::Admin,
//!         RethRpcModule::Debug,
//!         RethRpcModule::Eth,
//!         RethRpcModule::Web3,
//!     ]);
//!     let transport_modules = RpcModuleBuilder::new(
//!         provider,
//!         pool,
//!         network,
//!         TokioTaskExecutor::default(),
//!         events,
//!         evm_config,
//!     )
//!     .build(transports);
//!     let handle = RpcServerConfig::default()
//!         .with_http(ServerBuilder::default())
//!         .start(transport_modules)
//!         .await
//!         .unwrap();
//! }
//! ```
//!
//! Configure a http and ws server with a separate auth server that handles the `engine_` API
//!
//!
//! ```
//! use reth_engine_primitives::EngineTypes;
//! use reth_evm::ConfigureEvm;
//! use reth_network_api::{NetworkInfo, Peers};
//! use reth_provider::{
//!     AccountReader, BlockReaderIdExt, CanonStateSubscriptions, ChainSpecProvider,
//!     ChangeSetReader, EvmEnvProvider, StateProviderFactory,
//! };
//! use reth_rpc_api::EngineApiServer;
//! use reth_rpc_builder::{
//!     auth::AuthServerConfig, RethRpcModule, RpcModuleBuilder, RpcServerConfig,
//!     TransportRpcModuleConfig,
//! };
//! use reth_rpc_layer::JwtSecret;
//! use reth_tasks::TokioTaskExecutor;
//! use reth_transaction_pool::TransactionPool;
//! use tokio::try_join;
//! pub async fn launch<Provider, Pool, Network, Events, EngineApi, EngineT, EvmConfig>(
//!     provider: Provider,
//!     pool: Pool,
//!     network: Network,
//!     events: Events,
//!     engine_api: EngineApi,
//!     evm_config: EvmConfig,
//! ) where
//!     Provider: AccountReader
//!         + BlockReaderIdExt
//!         + ChainSpecProvider
//!         + ChangeSetReader
//!         + StateProviderFactory
//!         + EvmEnvProvider
//!         + Clone
//!         + Unpin
//!         + 'static,
//!     Pool: TransactionPool + Clone + 'static,
//!     Network: NetworkInfo + Peers + Clone + 'static,
//!     Events: CanonStateSubscriptions + Clone + 'static,
//!     EngineApi: EngineApiServer<EngineT>,
//!     EngineT: EngineTypes + 'static,
//!     EvmConfig: ConfigureEvm + 'static,
//! {
//!     // configure the rpc module per transport
//!     let transports = TransportRpcModuleConfig::default().with_http(vec![
//!         RethRpcModule::Admin,
//!         RethRpcModule::Debug,
//!         RethRpcModule::Eth,
//!         RethRpcModule::Web3,
//!     ]);
//!     let builder = RpcModuleBuilder::new(
//!         provider,
//!         pool,
//!         network,
//!         TokioTaskExecutor::default(),
//!         events,
//!         evm_config,
//!     );
//!
//!     // configure the server modules
//!     let (modules, auth_module, _registry) =
//!         builder.build_with_auth_server(transports, engine_api);
//!
//!     // start the servers
//!     let auth_config = AuthServerConfig::builder(JwtSecret::random()).build();
//!     let config = RpcServerConfig::default();
//!
//!     let (_rpc_handle, _auth_handle) =
//!         try_join!(modules.start_server(config), auth_module.start_server(auth_config),)
//!             .unwrap();
//! }
//! ```

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]

use crate::{
    auth::AuthRpcModule, cors::CorsDomainError, error::WsHttpSamePortError,
    metrics::RpcRequestMetrics,
};
use error::{ConflictingModules, RpcError, ServerKind};
use http::{header::AUTHORIZATION, HeaderMap};
use jsonrpsee::{
    core::RegisterMethodError,
    server::{AlreadyStoppedError, IdProvider, RpcServiceBuilder, Server, ServerHandle},
    Methods, RpcModule,
};
use reth_engine_primitives::EngineTypes;
use reth_evm::ConfigureEvm;
use reth_ipc::server::IpcServer;
use reth_network_api::{noop::NoopNetwork, NetworkInfo, Peers};
use reth_provider::{
    AccountReader, BlockReader, BlockReaderIdExt, CanonStateSubscriptions, ChainSpecProvider,
    ChangeSetReader, EvmEnvProvider, StateProviderFactory,
};
use reth_rpc::{
    eth::{
        cache::{cache_new_blocks_task, EthStateCache},
        fee_history_cache_new_blocks_task,
        gas_oracle::GasPriceOracle,
        traits::RawTransactionForwarder,
        EthBundle, FeeHistoryCache,
    },
    AdminApi, DebugApi, EngineEthApi, EthApi, EthFilter, EthPubSub, EthSubscriptionIdProvider,
    NetApi, OtterscanApi, RPCApi, RethApi, TraceApi, TxPoolApi, Web3Api,
};
use reth_rpc_api::servers::*;
use reth_rpc_layer::{AuthLayer, Claims, JwtAuthValidator, JwtSecret};
use reth_tasks::{
    pool::{BlockingTaskGuard, BlockingTaskPool},
    TaskSpawner, TokioTaskExecutor,
};
use reth_transaction_pool::{noop::NoopTransactionPool, TransactionPool};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fmt,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tower_http::cors::CorsLayer;
use tracing::{instrument, trace};

// re-export for convenience
pub use jsonrpsee::server::ServerBuilder;
pub use reth_ipc::server::{
    Builder as IpcServerBuilder, RpcServiceBuilder as IpcRpcServiceBuilder,
};
pub use reth_rpc_server_types::{constants, RethRpcModule, RpcModuleSelection};
pub use tower::layer::util::{Identity, Stack};

/// Auth server utilities.
pub mod auth;

/// RPC server utilities.
pub mod config;

/// Cors utilities.
mod cors;

/// Rpc error utilities.
pub mod error;

/// Eth utils
mod eth;
pub use eth::{EthConfig, EthHandlers};

// Rpc server metrics
mod metrics;

/// Convenience function for starting a server in one step.
#[allow(clippy::too_many_arguments)]
pub async fn launch<Provider, Pool, Network, Tasks, Events, EvmConfig>(
    provider: Provider,
    pool: Pool,
    network: Network,
    module_config: impl Into<TransportRpcModuleConfig>,
    server_config: impl Into<RpcServerConfig>,
    executor: Tasks,
    events: Events,
    evm_config: EvmConfig,
) -> Result<RpcServerHandle, RpcError>
where
    Provider: BlockReaderIdExt
        + AccountReader
        + StateProviderFactory
        + EvmEnvProvider
        + ChainSpecProvider
        + ChangeSetReader
        + Clone
        + Unpin
        + 'static,
    Pool: TransactionPool + Clone + 'static,
    Network: NetworkInfo + Peers + Clone + 'static,
    Tasks: TaskSpawner + Clone + 'static,
    Events: CanonStateSubscriptions + Clone + 'static,
    EvmConfig: ConfigureEvm + 'static,
{
    let module_config = module_config.into();
    let server_config = server_config.into();
    RpcModuleBuilder::new(provider, pool, network, executor, events, evm_config)
        .build(module_config)
        .start_server(server_config)
        .await
}

/// A builder type to configure the RPC module: See [`RpcModule`]
///
/// This is the main entrypoint and the easiest way to configure an RPC server.
#[derive(Debug, Clone)]
pub struct RpcModuleBuilder<Provider, Pool, Network, Tasks, Events, EvmConfig> {
    /// The Provider type to when creating all rpc handlers
    provider: Provider,
    /// The Pool type to when creating all rpc handlers
    pool: Pool,
    /// The Network type to when creating all rpc handlers
    network: Network,
    /// How additional tasks are spawned, for example in the eth pubsub namespace
    executor: Tasks,
    /// Provides access to chain events, such as new blocks, required by pubsub.
    events: Events,
    /// Defines how the EVM should be configured before execution.
    evm_config: EvmConfig,
}

// === impl RpcBuilder ===

impl<Provider, Pool, Network, Tasks, Events, EvmConfig>
    RpcModuleBuilder<Provider, Pool, Network, Tasks, Events, EvmConfig>
{
    /// Create a new instance of the builder
    pub const fn new(
        provider: Provider,
        pool: Pool,
        network: Network,
        executor: Tasks,
        events: Events,
        evm_config: EvmConfig,
    ) -> Self {
        Self { provider, pool, network, executor, events, evm_config }
    }

    /// Configure the provider instance.
    pub fn with_provider<P>(
        self,
        provider: P,
    ) -> RpcModuleBuilder<P, Pool, Network, Tasks, Events, EvmConfig>
    where
        P: BlockReader + StateProviderFactory + EvmEnvProvider + 'static,
    {
        let Self { pool, network, executor, events, evm_config, .. } = self;
        RpcModuleBuilder { provider, network, pool, executor, events, evm_config }
    }

    /// Configure the transaction pool instance.
    pub fn with_pool<P>(
        self,
        pool: P,
    ) -> RpcModuleBuilder<Provider, P, Network, Tasks, Events, EvmConfig>
    where
        P: TransactionPool + 'static,
    {
        let Self { provider, network, executor, events, evm_config, .. } = self;
        RpcModuleBuilder { provider, network, pool, executor, events, evm_config }
    }

    /// Configure a [`NoopTransactionPool`] instance.
    ///
    /// Caution: This will configure a pool API that does absolutely nothing.
    /// This is only intended for allow easier setup of namespaces that depend on the [`EthApi`]
    /// which requires a [`TransactionPool`] implementation.
    pub fn with_noop_pool(
        self,
    ) -> RpcModuleBuilder<Provider, NoopTransactionPool, Network, Tasks, Events, EvmConfig> {
        let Self { provider, executor, events, network, evm_config, .. } = self;
        RpcModuleBuilder {
            provider,
            executor,
            events,
            network,
            evm_config,
            pool: NoopTransactionPool::default(),
        }
    }

    /// Configure the network instance.
    pub fn with_network<N>(
        self,
        network: N,
    ) -> RpcModuleBuilder<Provider, Pool, N, Tasks, Events, EvmConfig>
    where
        N: NetworkInfo + Peers + 'static,
    {
        let Self { provider, pool, executor, events, evm_config, .. } = self;
        RpcModuleBuilder { provider, network, pool, executor, events, evm_config }
    }

    /// Configure a [`NoopNetwork`] instance.
    ///
    /// Caution: This will configure a network API that does absolutely nothing.
    /// This is only intended for allow easier setup of namespaces that depend on the [`EthApi`]
    /// which requires a [`NetworkInfo`] implementation.
    pub fn with_noop_network(
        self,
    ) -> RpcModuleBuilder<Provider, Pool, NoopNetwork, Tasks, Events, EvmConfig> {
        let Self { provider, pool, executor, events, evm_config, .. } = self;
        RpcModuleBuilder {
            provider,
            pool,
            executor,
            events,
            network: NoopNetwork::default(),
            evm_config,
        }
    }

    /// Configure the task executor to use for additional tasks.
    pub fn with_executor<T>(
        self,
        executor: T,
    ) -> RpcModuleBuilder<Provider, Pool, Network, T, Events, EvmConfig>
    where
        T: TaskSpawner + 'static,
    {
        let Self { pool, network, provider, events, evm_config, .. } = self;
        RpcModuleBuilder { provider, network, pool, executor, events, evm_config }
    }

    /// Configure [`TokioTaskExecutor`] as the task executor to use for additional tasks.
    ///
    /// This will spawn additional tasks directly via `tokio::task::spawn`, See
    /// [`TokioTaskExecutor`].
    pub fn with_tokio_executor(
        self,
    ) -> RpcModuleBuilder<Provider, Pool, Network, TokioTaskExecutor, Events, EvmConfig> {
        let Self { pool, network, provider, events, evm_config, .. } = self;
        RpcModuleBuilder {
            provider,
            network,
            pool,
            events,
            executor: TokioTaskExecutor::default(),
            evm_config,
        }
    }

    /// Configure the event subscriber instance
    pub fn with_events<E>(
        self,
        events: E,
    ) -> RpcModuleBuilder<Provider, Pool, Network, Tasks, E, EvmConfig>
    where
        E: CanonStateSubscriptions + 'static,
    {
        let Self { provider, pool, executor, network, evm_config, .. } = self;
        RpcModuleBuilder { provider, network, pool, executor, events, evm_config }
    }

    /// Configure the evm configuration type
    pub fn with_evm_config<E>(
        self,
        evm_config: E,
    ) -> RpcModuleBuilder<Provider, Pool, Network, Tasks, Events, E>
    where
        E: ConfigureEvm + 'static,
    {
        let Self { provider, pool, executor, network, events, .. } = self;
        RpcModuleBuilder { provider, network, pool, executor, events, evm_config }
    }
}

impl<Provider, Pool, Network, Tasks, Events, EvmConfig>
    RpcModuleBuilder<Provider, Pool, Network, Tasks, Events, EvmConfig>
where
    Provider: BlockReaderIdExt
        + AccountReader
        + StateProviderFactory
        + EvmEnvProvider
        + ChainSpecProvider
        + ChangeSetReader
        + Clone
        + Unpin
        + 'static,
    Pool: TransactionPool + Clone + 'static,
    Network: NetworkInfo + Peers + Clone + 'static,
    Tasks: TaskSpawner + Clone + 'static,
    Events: CanonStateSubscriptions + Clone + 'static,
    EvmConfig: ConfigureEvm + 'static,
{
    /// Configures all [`RpcModule`]s specific to the given [`TransportRpcModuleConfig`] which can
    /// be used to start the transport server(s).
    ///
    /// This behaves exactly as [`RpcModuleBuilder::build`] for the [`TransportRpcModules`], but
    /// also configures the auth (engine api) server, which exposes a subset of the `eth_`
    /// namespace.
    pub fn build_with_auth_server<EngineApi, EngineT>(
        self,
        module_config: TransportRpcModuleConfig,
        engine: EngineApi,
    ) -> (
        TransportRpcModules,
        AuthRpcModule,
        RethModuleRegistry<Provider, Pool, Network, Tasks, Events, EvmConfig>,
    )
    where
        EngineT: EngineTypes + 'static,
        EngineApi: EngineApiServer<EngineT>,
    {
        let Self { provider, pool, network, executor, events, evm_config } = self;

        let config = module_config.config.clone().unwrap_or_default();

        let mut registry =
            RethModuleRegistry::new(provider, pool, network, executor, events, config, evm_config);

        let modules = registry.create_transport_rpc_modules(module_config);

        let auth_module = registry.create_auth_module(engine);

        (modules, auth_module, registry)
    }

    /// Converts the builder into a [`RethModuleRegistry`] which can be used to create all
    /// components.
    ///
    /// This is useful for getting access to API handlers directly:
    ///
    /// # Example
    ///
    /// ```no_run
    /// use reth_evm::ConfigureEvm;
    /// use reth_network_api::noop::NoopNetwork;
    /// use reth_provider::test_utils::{NoopProvider, TestCanonStateSubscriptions};
    /// use reth_rpc_builder::RpcModuleBuilder;
    /// use reth_tasks::TokioTaskExecutor;
    /// use reth_transaction_pool::noop::NoopTransactionPool;
    ///
    /// fn init<Evm: ConfigureEvm + 'static>(evm: Evm) {
    ///     let mut registry = RpcModuleBuilder::default()
    ///         .with_provider(NoopProvider::default())
    ///         .with_pool(NoopTransactionPool::default())
    ///         .with_network(NoopNetwork::default())
    ///         .with_executor(TokioTaskExecutor::default())
    ///         .with_events(TestCanonStateSubscriptions::default())
    ///         .with_evm_config(evm)
    ///         .into_registry(Default::default());
    ///
    ///     let eth_api = registry.eth_api();
    /// }
    /// ```
    pub fn into_registry(
        self,
        config: RpcModuleConfig,
    ) -> RethModuleRegistry<Provider, Pool, Network, Tasks, Events, EvmConfig> {
        let Self { provider, pool, network, executor, events, evm_config } = self;
        RethModuleRegistry::new(provider, pool, network, executor, events, config, evm_config)
    }

    /// Configures all [`RpcModule`]s specific to the given [`TransportRpcModuleConfig`] which can
    /// be used to start the transport server(s).
    ///
    /// See also [`RpcServer::start`]
    pub fn build(self, module_config: TransportRpcModuleConfig) -> TransportRpcModules<()> {
        let mut modules = TransportRpcModules::default();

        let Self { provider, pool, network, executor, events, evm_config } = self;

        if !module_config.is_empty() {
            let TransportRpcModuleConfig { http, ws, ipc, config } = module_config.clone();

            let mut registry = RethModuleRegistry::new(
                provider,
                pool,
                network,
                executor,
                events,
                config.unwrap_or_default(),
                evm_config,
            );

            modules.config = module_config;
            modules.http = registry.maybe_module(http.as_ref());
            modules.ws = registry.maybe_module(ws.as_ref());
            modules.ipc = registry.maybe_module(ipc.as_ref());
        }

        modules
    }
}

impl Default for RpcModuleBuilder<(), (), (), (), (), ()> {
    fn default() -> Self {
        Self::new((), (), (), (), (), ())
    }
}

/// Bundles settings for modules
#[derive(Debug, Default, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct RpcModuleConfig {
    /// `eth` namespace settings
    eth: EthConfig,
}

// === impl RpcModuleConfig ===

impl RpcModuleConfig {
    /// Convenience method to create a new [`RpcModuleConfigBuilder`]
    pub fn builder() -> RpcModuleConfigBuilder {
        RpcModuleConfigBuilder::default()
    }

    /// Returns a new RPC module config given the eth namespace config
    pub const fn new(eth: EthConfig) -> Self {
        Self { eth }
    }

    /// Get a reference to the eth namespace config
    pub const fn eth(&self) -> &EthConfig {
        &self.eth
    }

    /// Get a mutable reference to the eth namespace config
    pub fn eth_mut(&mut self) -> &mut EthConfig {
        &mut self.eth
    }
}

/// Configures [`RpcModuleConfig`]
#[derive(Clone, Debug, Default)]
pub struct RpcModuleConfigBuilder {
    eth: Option<EthConfig>,
}

// === impl RpcModuleConfigBuilder ===

impl RpcModuleConfigBuilder {
    /// Configures a custom eth namespace config
    pub const fn eth(mut self, eth: EthConfig) -> Self {
        self.eth = Some(eth);
        self
    }

    /// Consumes the type and creates the [`RpcModuleConfig`]
    pub fn build(self) -> RpcModuleConfig {
        let Self { eth } = self;
        RpcModuleConfig { eth: eth.unwrap_or_default() }
    }

    /// Get a reference to the eth namespace config, if any
    pub const fn get_eth(&self) -> &Option<EthConfig> {
        &self.eth
    }

    /// Get a mutable reference to the eth namespace config, if any
    pub fn eth_mut(&mut self) -> &mut Option<EthConfig> {
        &mut self.eth
    }

    /// Get the eth namespace config, creating a default if none is set
    pub fn eth_mut_or_default(&mut self) -> &mut EthConfig {
        self.eth.get_or_insert_with(EthConfig::default)
    }
}

/// A Helper type the holds instances of the configured modules.
#[derive(Debug, Clone)]
pub struct RethModuleRegistry<Provider, Pool, Network, Tasks, Events, EvmConfig> {
    provider: Provider,
    pool: Pool,
    network: Network,
    executor: Tasks,
    events: Events,
    /// Defines how to configure the EVM before execution.
    evm_config: EvmConfig,
    /// Additional settings for handlers.
    config: RpcModuleConfig,
    /// Holds a clone of all the eth namespace handlers
    eth: Option<EthHandlers<Provider, Pool, Network, Events, EvmConfig>>,
    /// to put trace calls behind semaphore
    blocking_pool_guard: BlockingTaskGuard,
    /// Contains the [Methods] of a module
    modules: HashMap<RethRpcModule, Methods>,
    /// Optional forwarder for `eth_sendRawTransaction`
    // TODO(mattsse): find a more ergonomic way to configure eth/rpc customizations
    eth_raw_transaction_forwarder: Option<Arc<dyn RawTransactionForwarder>>,
}

// === impl RethModuleRegistry ===

impl<Provider, Pool, Network, Tasks, Events, EvmConfig>
    RethModuleRegistry<Provider, Pool, Network, Tasks, Events, EvmConfig>
{
    /// Creates a new, empty instance.
    pub fn new(
        provider: Provider,
        pool: Pool,
        network: Network,
        executor: Tasks,
        events: Events,
        config: RpcModuleConfig,
        evm_config: EvmConfig,
    ) -> Self {
        Self {
            provider,
            pool,
            network,
            evm_config,
            eth: None,
            executor,
            modules: Default::default(),
            blocking_pool_guard: BlockingTaskGuard::new(config.eth.max_tracing_requests),
            config,
            events,
            eth_raw_transaction_forwarder: None,
        }
    }

    /// Sets a forwarder for `eth_sendRawTransaction`
    ///
    /// Note: this might be removed in the future in favor of a more generic approach.
    pub fn set_eth_raw_transaction_forwarder(
        &mut self,
        forwarder: Arc<dyn RawTransactionForwarder>,
    ) {
        if let Some(eth) = self.eth.as_ref() {
            // in case the eth api has been created before the forwarder was set: <https://github.com/paradigmxyz/reth/issues/8661>
            eth.api.set_eth_raw_transaction_forwarder(forwarder.clone());
        }
        self.eth_raw_transaction_forwarder = Some(forwarder);
    }

    /// Returns a reference to the pool
    pub const fn pool(&self) -> &Pool {
        &self.pool
    }

    /// Returns a reference to the events type
    pub const fn events(&self) -> &Events {
        &self.events
    }

    /// Returns a reference to the tasks type
    pub const fn tasks(&self) -> &Tasks {
        &self.executor
    }

    /// Returns a reference to the provider
    pub const fn provider(&self) -> &Provider {
        &self.provider
    }

    /// Returns all installed methods
    pub fn methods(&self) -> Vec<Methods> {
        self.modules.values().cloned().collect()
    }

    /// Returns a merged `RpcModule`
    pub fn module(&self) -> RpcModule<()> {
        let mut module = RpcModule::new(());
        for methods in self.modules.values().cloned() {
            module.merge(methods).expect("No conflicts");
        }
        module
    }
}

impl<Provider: ChainSpecProvider, Pool, Network, Tasks, Events, EvmConfig>
    RethModuleRegistry<Provider, Pool, Network, Tasks, Events, EvmConfig>
where
    Network: NetworkInfo + Peers + Clone + 'static,
{
    /// Instantiates `AdminApi`
    pub fn admin_api(&self) -> AdminApi<Network> {
        AdminApi::new(self.network.clone(), self.provider.chain_spec())
    }

    /// Instantiates `Web3Api`
    pub fn web3_api(&self) -> Web3Api<Network> {
        Web3Api::new(self.network.clone())
    }

    /// Register Admin Namespace
    pub fn register_admin(&mut self) -> &mut Self {
        let adminapi = self.admin_api();
        self.modules.insert(RethRpcModule::Admin, adminapi.into_rpc().into());
        self
    }

    /// Register Web3 Namespace
    pub fn register_web3(&mut self) -> &mut Self {
        let web3api = self.web3_api();
        self.modules.insert(RethRpcModule::Web3, web3api.into_rpc().into());
        self
    }
}

impl<Provider, Pool, Network, Tasks, Events, EvmConfig>
    RethModuleRegistry<Provider, Pool, Network, Tasks, Events, EvmConfig>
where
    Provider: BlockReaderIdExt
        + AccountReader
        + StateProviderFactory
        + EvmEnvProvider
        + ChainSpecProvider
        + ChangeSetReader
        + Clone
        + Unpin
        + 'static,
    Pool: TransactionPool + Clone + 'static,
    Network: NetworkInfo + Peers + Clone + 'static,
    Tasks: TaskSpawner + Clone + 'static,
    Events: CanonStateSubscriptions + Clone + 'static,
    EvmConfig: ConfigureEvm + 'static,
{
    /// Register Eth Namespace
    ///
    /// # Panics
    ///
    /// If called outside of the tokio runtime. See also [`Self::eth_api`]
    pub fn register_eth(&mut self) -> &mut Self {
        let eth_api = self.eth_api();
        self.modules.insert(RethRpcModule::Eth, eth_api.into_rpc().into());
        self
    }

    /// Register Otterscan Namespace
    ///
    /// # Panics
    ///
    /// If called outside of the tokio runtime. See also [`Self::eth_api`]
    pub fn register_ots(&mut self) -> &mut Self {
        let otterscan_api = self.otterscan_api();
        self.modules.insert(RethRpcModule::Ots, otterscan_api.into_rpc().into());
        self
    }

    /// Register Debug Namespace
    ///
    /// # Panics
    ///
    /// If called outside of the tokio runtime. See also [`Self::eth_api`]
    pub fn register_debug(&mut self) -> &mut Self {
        let debug_api = self.debug_api();
        self.modules.insert(RethRpcModule::Debug, debug_api.into_rpc().into());
        self
    }

    /// Register Trace Namespace
    ///
    /// # Panics
    ///
    /// If called outside of the tokio runtime. See also [`Self::eth_api`]
    pub fn register_trace(&mut self) -> &mut Self {
        let trace_api = self.trace_api();
        self.modules.insert(RethRpcModule::Trace, trace_api.into_rpc().into());
        self
    }

    /// Configures the auth module that includes the
    ///   * `engine_` namespace
    ///   * `api_` namespace
    ///
    /// Note: This does _not_ register the `engine_` in this registry.
    pub fn create_auth_module<EngineApi, EngineT>(&mut self, engine_api: EngineApi) -> AuthRpcModule
    where
        EngineT: EngineTypes + 'static,
        EngineApi: EngineApiServer<EngineT>,
    {
        let eth_handlers = self.eth_handlers();
        let mut module = RpcModule::new(());

        module.merge(engine_api.into_rpc()).expect("No conflicting methods");

        // also merge a subset of `eth_` handlers
        let engine_eth = EngineEthApi::new(eth_handlers.api.clone(), eth_handlers.filter);
        module.merge(engine_eth.into_rpc()).expect("No conflicting methods");

        AuthRpcModule { inner: module }
    }

    /// Register Net Namespace
    ///
    /// See also [`Self::eth_api`]
    ///
    /// # Panics
    ///
    /// If called outside of the tokio runtime.
    pub fn register_net(&mut self) -> &mut Self {
        let netapi = self.net_api();
        self.modules.insert(RethRpcModule::Net, netapi.into_rpc().into());
        self
    }

    /// Register Reth namespace
    ///
    /// See also [`Self::eth_api`]
    ///
    /// # Panics
    ///
    /// If called outside of the tokio runtime.
    pub fn register_reth(&mut self) -> &mut Self {
        let rethapi = self.reth_api();
        self.modules.insert(RethRpcModule::Reth, rethapi.into_rpc().into());
        self
    }

    /// Helper function to create a [`RpcModule`] if it's not `None`
    fn maybe_module(&mut self, config: Option<&RpcModuleSelection>) -> Option<RpcModule<()>> {
        config.map(|config| self.module_for(config))
    }

    /// Configure a [`TransportRpcModules`] using the current registry. This
    /// creates [`RpcModule`] instances for the modules selected by the
    /// `config`.
    pub fn create_transport_rpc_modules(
        &mut self,
        config: TransportRpcModuleConfig,
    ) -> TransportRpcModules<()> {
        let mut modules = TransportRpcModules::default();
        let http = self.maybe_module(config.http.as_ref());
        let ws = self.maybe_module(config.ws.as_ref());
        let ipc = self.maybe_module(config.ipc.as_ref());

        modules.config = config;
        modules.http = http;
        modules.ws = ws;
        modules.ipc = ipc;
        modules
    }

    /// Populates a new [`RpcModule`] based on the selected [`RethRpcModule`]s in the given
    /// [`RpcModuleSelection`]
    pub fn module_for(&mut self, config: &RpcModuleSelection) -> RpcModule<()> {
        let mut module = RpcModule::new(());
        let all_methods = self.reth_methods(config.iter_selection());
        for methods in all_methods {
            module.merge(methods).expect("No conflicts");
        }
        module
    }

    /// Returns the [Methods] for the given [`RethRpcModule`]
    ///
    /// If this is the first time the namespace is requested, a new instance of API implementation
    /// will be created.
    ///
    /// # Panics
    ///
    /// If called outside of the tokio runtime. See also [`Self::eth_api`]
    pub fn reth_methods(
        &mut self,
        namespaces: impl Iterator<Item = RethRpcModule>,
    ) -> Vec<Methods> {
        let EthHandlers {
            api: eth_api,
            filter: eth_filter,
            pubsub: eth_pubsub,
            cache: _,
            blocking_task_pool: _,
        } = self.with_eth(|eth| eth.clone());

        // Create a copy, so we can list out all the methods for rpc_ api
        let namespaces: Vec<_> = namespaces.collect();
        namespaces
            .iter()
            .copied()
            .map(|namespace| {
                self.modules
                    .entry(namespace)
                    .or_insert_with(|| match namespace {
                        RethRpcModule::Admin => {
                            AdminApi::new(self.network.clone(), self.provider.chain_spec())
                                .into_rpc()
                                .into()
                        }
                        RethRpcModule::Debug => DebugApi::new(
                            self.provider.clone(),
                            eth_api.clone(),
                            self.blocking_pool_guard.clone(),
                        )
                        .into_rpc()
                        .into(),
                        RethRpcModule::Eth => {
                            // merge all eth handlers
                            let mut module = eth_api.clone().into_rpc();
                            module.merge(eth_filter.clone().into_rpc()).expect("No conflicts");
                            module.merge(eth_pubsub.clone().into_rpc()).expect("No conflicts");

                            module.into()
                        }
                        RethRpcModule::Net => {
                            NetApi::new(self.network.clone(), eth_api.clone()).into_rpc().into()
                        }
                        RethRpcModule::Trace => TraceApi::new(
                            self.provider.clone(),
                            eth_api.clone(),
                            self.blocking_pool_guard.clone(),
                        )
                        .into_rpc()
                        .into(),
                        RethRpcModule::Web3 => Web3Api::new(self.network.clone()).into_rpc().into(),
                        RethRpcModule::Txpool => {
                            TxPoolApi::new(self.pool.clone()).into_rpc().into()
                        }
                        RethRpcModule::Rpc => RPCApi::new(
                            namespaces
                                .iter()
                                .map(|module| (module.to_string(), "1.0".to_string()))
                                .collect(),
                        )
                        .into_rpc()
                        .into(),
                        RethRpcModule::Ots => OtterscanApi::new(eth_api.clone()).into_rpc().into(),
                        RethRpcModule::Reth => {
                            RethApi::new(self.provider.clone(), Box::new(self.executor.clone()))
                                .into_rpc()
                                .into()
                        }
                        RethRpcModule::EthCallBundle => {
                            EthBundle::new(eth_api.clone(), self.blocking_pool_guard.clone())
                                .into_rpc()
                                .into()
                        }
                    })
                    .clone()
            })
            .collect::<Vec<_>>()
    }

    /// Returns the [`EthStateCache`] frontend
    ///
    /// This will spawn exactly one [`EthStateCache`] service if this is the first time the cache is
    /// requested.
    pub fn eth_cache(&mut self) -> EthStateCache {
        self.with_eth(|handlers| handlers.cache.clone())
    }

    /// Creates the [`EthHandlers`] type the first time this is called.
    ///
    /// This will spawn the required service tasks for [`EthApi`] for:
    ///   - [`EthStateCache`]
    ///   - [`FeeHistoryCache`]
    fn with_eth<F, R>(&mut self, f: F) -> R
    where
        F: FnOnce(&EthHandlers<Provider, Pool, Network, Events, EvmConfig>) -> R,
    {
        f(match &self.eth {
            Some(eth) => eth,
            None => self.eth.insert(self.init_eth()),
        })
    }

    fn init_eth(&self) -> EthHandlers<Provider, Pool, Network, Events, EvmConfig> {
        let cache = EthStateCache::spawn_with(
            self.provider.clone(),
            self.config.eth.cache.clone(),
            self.executor.clone(),
            self.evm_config.clone(),
        );
        let gas_oracle = GasPriceOracle::new(
            self.provider.clone(),
            self.config.eth.gas_oracle.clone(),
            cache.clone(),
        );
        let new_canonical_blocks = self.events.canonical_state_stream();
        let c = cache.clone();

        self.executor.spawn_critical(
            "cache canonical blocks task",
            Box::pin(async move {
                cache_new_blocks_task(c, new_canonical_blocks).await;
            }),
        );

        let fee_history_cache =
            FeeHistoryCache::new(cache.clone(), self.config.eth.fee_history_cache.clone());
        let new_canonical_blocks = self.events.canonical_state_stream();
        let fhc = fee_history_cache.clone();
        let provider_clone = self.provider.clone();
        self.executor.spawn_critical(
            "cache canonical blocks for fee history task",
            Box::pin(async move {
                fee_history_cache_new_blocks_task(fhc, new_canonical_blocks, provider_clone).await;
            }),
        );

        let executor = Box::new(self.executor.clone());
        let blocking_task_pool = BlockingTaskPool::build().expect("failed to build tracing pool");
        let api = EthApi::with_spawner(
            self.provider.clone(),
            self.pool.clone(),
            self.network.clone(),
            cache.clone(),
            gas_oracle,
            self.config.eth.rpc_gas_cap,
            executor.clone(),
            blocking_task_pool.clone(),
            fee_history_cache,
            self.evm_config.clone(),
            self.eth_raw_transaction_forwarder.clone(),
        );
        let filter = EthFilter::new(
            self.provider.clone(),
            self.pool.clone(),
            cache.clone(),
            self.config.eth.filter_config(),
            executor.clone(),
        );

        let pubsub = EthPubSub::with_spawner(
            self.provider.clone(),
            self.pool.clone(),
            self.events.clone(),
            self.network.clone(),
            executor,
        );

        EthHandlers { api, cache, filter, pubsub, blocking_task_pool }
    }

    /// Returns the configured [`EthHandlers`] or creates it if it does not exist yet
    ///
    /// # Panics
    ///
    /// If called outside of the tokio runtime. See also [`Self::eth_api`]
    pub fn eth_handlers(&mut self) -> EthHandlers<Provider, Pool, Network, Events, EvmConfig> {
        self.with_eth(|handlers| handlers.clone())
    }

    /// Returns the configured [`EthApi`] or creates it if it does not exist yet
    ///
    /// Caution: This will spawn the necessary tasks required by the [`EthApi`]: [`EthStateCache`].
    ///
    /// # Panics
    ///
    /// If called outside of the tokio runtime.
    pub fn eth_api(&mut self) -> EthApi<Provider, Pool, Network, EvmConfig> {
        self.with_eth(|handlers| handlers.api.clone())
    }

    /// Instantiates `TraceApi`
    ///
    /// # Panics
    ///
    /// If called outside of the tokio runtime. See also [`Self::eth_api`]
    pub fn trace_api(&mut self) -> TraceApi<Provider, EthApi<Provider, Pool, Network, EvmConfig>> {
        let eth = self.eth_handlers();
        TraceApi::new(self.provider.clone(), eth.api, self.blocking_pool_guard.clone())
    }

    /// Instantiates [`EthBundle`] Api
    ///
    /// # Panics
    ///
    /// If called outside of the tokio runtime. See also [`Self::eth_api`]
    pub fn bundle_api(&mut self) -> EthBundle<EthApi<Provider, Pool, Network, EvmConfig>> {
        let eth_api = self.eth_api();
        EthBundle::new(eth_api, self.blocking_pool_guard.clone())
    }

    /// Instantiates `OtterscanApi`
    ///
    /// # Panics
    ///
    /// If called outside of the tokio runtime. See also [`Self::eth_api`]
    pub fn otterscan_api(&mut self) -> OtterscanApi<EthApi<Provider, Pool, Network, EvmConfig>> {
        let eth_api = self.eth_api();
        OtterscanApi::new(eth_api)
    }

    /// Instantiates `DebugApi`
    ///
    /// # Panics
    ///
    /// If called outside of the tokio runtime. See also [`Self::eth_api`]
    pub fn debug_api(&mut self) -> DebugApi<Provider, EthApi<Provider, Pool, Network, EvmConfig>> {
        let eth_api = self.eth_api();
        DebugApi::new(self.provider.clone(), eth_api, self.blocking_pool_guard.clone())
    }

    /// Instantiates `NetApi`
    ///
    /// # Panics
    ///
    /// If called outside of the tokio runtime. See also [`Self::eth_api`]
    pub fn net_api(&mut self) -> NetApi<Network, EthApi<Provider, Pool, Network, EvmConfig>> {
        let eth_api = self.eth_api();
        NetApi::new(self.network.clone(), eth_api)
    }

    /// Instantiates `RethApi`
    pub fn reth_api(&self) -> RethApi<Provider> {
        RethApi::new(self.provider.clone(), Box::new(self.executor.clone()))
    }
}

/// A builder type for configuring and launching the servers that will handle RPC requests.
///
/// Supported server transports are:
///    - http
///    - ws
///    - ipc
///
/// Http and WS share the same settings: [`ServerBuilder`].
///
/// Once the [`RpcModule`] is built via [`RpcModuleBuilder`] the servers can be started, See also
/// [`ServerBuilder::build`] and [`Server::start`](jsonrpsee::server::Server::start).
#[derive(Default, Debug)]
pub struct RpcServerConfig {
    /// Configs for JSON-RPC Http.
    http_server_config: Option<ServerBuilder<Identity, Identity>>,
    /// Allowed CORS Domains for http
    http_cors_domains: Option<String>,
    /// Address where to bind the http server to
    http_addr: Option<SocketAddr>,
    /// Configs for WS server
    ws_server_config: Option<ServerBuilder<Identity, Identity>>,
    /// Allowed CORS Domains for ws.
    ws_cors_domains: Option<String>,
    /// Address where to bind the ws server to
    ws_addr: Option<SocketAddr>,
    /// Configs for JSON-RPC IPC server
    ipc_server_config: Option<IpcServerBuilder<Identity, Identity>>,
    /// The Endpoint where to launch the ipc server
    ipc_endpoint: Option<String>,
    /// JWT secret for authentication
    jwt_secret: Option<JwtSecret>,
}

// === impl RpcServerConfig ===

impl RpcServerConfig {
    /// Creates a new config with only http set
    pub fn http(config: ServerBuilder<Identity, Identity>) -> Self {
        Self::default().with_http(config)
    }

    /// Creates a new config with only ws set
    pub fn ws(config: ServerBuilder<Identity, Identity>) -> Self {
        Self::default().with_ws(config)
    }

    /// Creates a new config with only ipc set
    pub fn ipc(config: IpcServerBuilder<Identity, Identity>) -> Self {
        Self::default().with_ipc(config)
    }

    /// Configures the http server
    ///
    /// Note: this always configures an [`EthSubscriptionIdProvider`] [`IdProvider`] for
    /// convenience. To set a custom [`IdProvider`], please use [`Self::with_id_provider`].
    pub fn with_http(mut self, config: ServerBuilder<Identity, Identity>) -> Self {
        self.http_server_config =
            Some(config.set_id_provider(EthSubscriptionIdProvider::default()));
        self
    }

    /// Configure the cors domains for http _and_ ws
    pub fn with_cors(self, cors_domain: Option<String>) -> Self {
        self.with_http_cors(cors_domain.clone()).with_ws_cors(cors_domain)
    }

    /// Configure the cors domains for HTTP
    pub fn with_http_cors(mut self, cors_domain: Option<String>) -> Self {
        self.http_cors_domains = cors_domain;
        self
    }

    /// Configure the cors domains for WS
    pub fn with_ws_cors(mut self, cors_domain: Option<String>) -> Self {
        self.ws_cors_domains = cors_domain;
        self
    }

    /// Configures the ws server
    ///
    /// Note: this always configures an [`EthSubscriptionIdProvider`] [`IdProvider`] for
    /// convenience. To set a custom [`IdProvider`], please use [`Self::with_id_provider`].
    pub fn with_ws(mut self, config: ServerBuilder<Identity, Identity>) -> Self {
        self.ws_server_config = Some(config.set_id_provider(EthSubscriptionIdProvider::default()));
        self
    }

    /// Configures the [`SocketAddr`] of the http server
    ///
    /// Default is [`Ipv4Addr::LOCALHOST`] and
    /// [`reth_rpc_server_types::constants::DEFAULT_HTTP_RPC_PORT`]
    pub const fn with_http_address(mut self, addr: SocketAddr) -> Self {
        self.http_addr = Some(addr);
        self
    }

    /// Configures the [`SocketAddr`] of the ws server
    ///
    /// Default is [`Ipv4Addr::LOCALHOST`] and
    /// [`reth_rpc_server_types::constants::DEFAULT_WS_RPC_PORT`]
    pub const fn with_ws_address(mut self, addr: SocketAddr) -> Self {
        self.ws_addr = Some(addr);
        self
    }

    /// Configures the ipc server
    ///
    /// Note: this always configures an [`EthSubscriptionIdProvider`] [`IdProvider`] for
    /// convenience. To set a custom [`IdProvider`], please use [`Self::with_id_provider`].
    pub fn with_ipc(mut self, config: IpcServerBuilder<Identity, Identity>) -> Self {
        self.ipc_server_config = Some(config.set_id_provider(EthSubscriptionIdProvider::default()));
        self
    }

    /// Sets a custom [`IdProvider`] for all configured transports.
    ///
    /// By default all transports use [`EthSubscriptionIdProvider`]
    pub fn with_id_provider<I>(mut self, id_provider: I) -> Self
    where
        I: IdProvider + Clone + 'static,
    {
        if let Some(http) = self.http_server_config {
            self.http_server_config = Some(http.set_id_provider(id_provider.clone()));
        }
        if let Some(ws) = self.ws_server_config {
            self.ws_server_config = Some(ws.set_id_provider(id_provider.clone()));
        }
        if let Some(ipc) = self.ipc_server_config {
            self.ipc_server_config = Some(ipc.set_id_provider(id_provider));
        }

        self
    }

    /// Configures the endpoint of the ipc server
    ///
    /// Default is [`reth_rpc_server_types::constants::DEFAULT_IPC_ENDPOINT`]
    pub fn with_ipc_endpoint(mut self, path: impl Into<String>) -> Self {
        self.ipc_endpoint = Some(path.into());
        self
    }

    /// Configures the JWT secret for authentication.
    pub const fn with_jwt_secret(mut self, secret: Option<JwtSecret>) -> Self {
        self.jwt_secret = secret;
        self
    }

    /// Returns true if any server is configured.
    ///
    /// If no server is configured, no server will be be launched on [`RpcServerConfig::start`].
    pub const fn has_server(&self) -> bool {
        self.http_server_config.is_some() ||
            self.ws_server_config.is_some() ||
            self.ipc_server_config.is_some()
    }

    /// Returns the [`SocketAddr`] of the http server
    pub const fn http_address(&self) -> Option<SocketAddr> {
        self.http_addr
    }

    /// Returns the [`SocketAddr`] of the ws server
    pub const fn ws_address(&self) -> Option<SocketAddr> {
        self.ws_addr
    }

    /// Returns the endpoint of the ipc server
    pub fn ipc_endpoint(&self) -> Option<String> {
        self.ipc_endpoint.clone()
    }

    /// Convenience function to do [`RpcServerConfig::build`] and [`RpcServer::start`] in one step
    pub async fn start(self, modules: TransportRpcModules) -> Result<RpcServerHandle, RpcError> {
        self.build(&modules).await?.start(modules).await
    }

    /// Creates the [`CorsLayer`] if any
    fn maybe_cors_layer(cors: Option<String>) -> Result<Option<CorsLayer>, CorsDomainError> {
        cors.as_deref().map(cors::create_cors_layer).transpose()
    }

    /// Creates the [`AuthLayer`] if any
    fn maybe_jwt_layer(&self) -> Option<AuthLayer<JwtAuthValidator>> {
        self.jwt_secret.map(|secret| AuthLayer::new(JwtAuthValidator::new(secret)))
    }

    /// Builds the ws and http server(s).
    ///
    /// If both are on the same port, they are combined into one server.
    async fn build_ws_http(
        &mut self,
        modules: &TransportRpcModules,
    ) -> Result<WsHttpServer, RpcError> {
        let http_socket_addr = self.http_addr.unwrap_or(SocketAddr::V4(SocketAddrV4::new(
            Ipv4Addr::LOCALHOST,
            constants::DEFAULT_HTTP_RPC_PORT,
        )));

        let ws_socket_addr = self.ws_addr.unwrap_or(SocketAddr::V4(SocketAddrV4::new(
            Ipv4Addr::LOCALHOST,
            constants::DEFAULT_WS_RPC_PORT,
        )));

        // If both are configured on the same port, we combine them into one server.
        if self.http_addr == self.ws_addr &&
            self.http_server_config.is_some() &&
            self.ws_server_config.is_some()
        {
            let cors = match (self.ws_cors_domains.as_ref(), self.http_cors_domains.as_ref()) {
                (Some(ws_cors), Some(http_cors)) => {
                    if ws_cors.trim() != http_cors.trim() {
                        return Err(WsHttpSamePortError::ConflictingCorsDomains {
                            http_cors_domains: Some(http_cors.clone()),
                            ws_cors_domains: Some(ws_cors.clone()),
                        }
                        .into());
                    }
                    Some(ws_cors)
                }
                (a, b) => a.or(b),
            }
            .cloned();

            // we merge this into one server using the http setup
            self.ws_server_config.take();

            modules.config.ensure_ws_http_identical()?;

            let builder = self.http_server_config.take().expect("http_server_config is Some");
            let server = builder
                .set_http_middleware(
                    tower::ServiceBuilder::new()
                        .option_layer(Self::maybe_cors_layer(cors)?)
                        .option_layer(self.maybe_jwt_layer()),
                )
                .set_rpc_middleware(
                    RpcServiceBuilder::new().layer(
                        modules
                            .http
                            .as_ref()
                            .or(modules.ws.as_ref())
                            .map(RpcRequestMetrics::same_port)
                            .unwrap_or_default(),
                    ),
                )
                .build(http_socket_addr)
                .await
                .map_err(|err| RpcError::server_error(err, ServerKind::WsHttp(http_socket_addr)))?;
            let addr = server
                .local_addr()
                .map_err(|err| RpcError::server_error(err, ServerKind::WsHttp(http_socket_addr)))?;
            return Ok(WsHttpServer {
                http_local_addr: Some(addr),
                ws_local_addr: Some(addr),
                server: WsHttpServers::SamePort(server),
                jwt_secret: self.jwt_secret,
            });
        }

        let mut http_local_addr = None;
        let mut http_server = None;

        let mut ws_local_addr = None;
        let mut ws_server = None;
        if let Some(builder) = self.ws_server_config.take() {
            let server = builder
                .ws_only()
                .set_http_middleware(
                    tower::ServiceBuilder::new()
                        .option_layer(Self::maybe_cors_layer(self.ws_cors_domains.clone())?)
                        .option_layer(self.maybe_jwt_layer()),
                )
                .set_rpc_middleware(
                    RpcServiceBuilder::new()
                        .layer(modules.ws.as_ref().map(RpcRequestMetrics::ws).unwrap_or_default()),
                )
                .build(ws_socket_addr)
                .await
                .map_err(|err| RpcError::server_error(err, ServerKind::WS(ws_socket_addr)))?;
            let addr = server
                .local_addr()
                .map_err(|err| RpcError::server_error(err, ServerKind::WS(ws_socket_addr)))?;

            ws_local_addr = Some(addr);
            ws_server = Some(server);
        }

        if let Some(builder) = self.http_server_config.take() {
            let server = builder
                .http_only()
                .set_http_middleware(
                    tower::ServiceBuilder::new()
                        .option_layer(Self::maybe_cors_layer(self.http_cors_domains.clone())?)
                        .option_layer(self.maybe_jwt_layer()),
                )
                .set_rpc_middleware(
                    RpcServiceBuilder::new().layer(
                        modules.http.as_ref().map(RpcRequestMetrics::http).unwrap_or_default(),
                    ),
                )
                .build(http_socket_addr)
                .await
                .map_err(|err| RpcError::server_error(err, ServerKind::Http(http_socket_addr)))?;
            let local_addr = server
                .local_addr()
                .map_err(|err| RpcError::server_error(err, ServerKind::Http(http_socket_addr)))?;
            http_local_addr = Some(local_addr);
            http_server = Some(server);
        }

        Ok(WsHttpServer {
            http_local_addr,
            ws_local_addr,
            server: WsHttpServers::DifferentPort { http: http_server, ws: ws_server },
            jwt_secret: self.jwt_secret,
        })
    }

    /// Finalize the configuration of the server(s).
    ///
    /// This consumes the builder and returns a server.
    ///
    /// Note: The server is not started and does nothing unless polled, See also
    /// [`RpcServer::start`]
    pub async fn build(mut self, modules: &TransportRpcModules) -> Result<RpcServer, RpcError> {
        let mut server = RpcServer::empty();
        server.ws_http = self.build_ws_http(modules).await?;

        if let Some(builder) = self.ipc_server_config {
            let metrics = modules.ipc.as_ref().map(RpcRequestMetrics::ipc).unwrap_or_default();
            let ipc_path =
                self.ipc_endpoint.unwrap_or_else(|| constants::DEFAULT_IPC_ENDPOINT.into());
            let ipc = builder
                .set_rpc_middleware(IpcRpcServiceBuilder::new().layer(metrics))
                .build(ipc_path);
            server.ipc = Some(ipc);
        }

        Ok(server)
    }
}

/// Holds modules to be installed per transport type
///
/// # Example
///
/// Configure a http transport only
///
/// ```
/// use reth_rpc_builder::{RethRpcModule, TransportRpcModuleConfig};
/// let config =
///     TransportRpcModuleConfig::default().with_http([RethRpcModule::Eth, RethRpcModule::Admin]);
/// ```
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct TransportRpcModuleConfig {
    /// http module configuration
    http: Option<RpcModuleSelection>,
    /// ws module configuration
    ws: Option<RpcModuleSelection>,
    /// ipc module configuration
    ipc: Option<RpcModuleSelection>,
    /// Config for the modules
    config: Option<RpcModuleConfig>,
}

// === impl TransportRpcModuleConfig ===

impl TransportRpcModuleConfig {
    /// Creates a new config with only http set
    pub fn set_http(http: impl Into<RpcModuleSelection>) -> Self {
        Self::default().with_http(http)
    }

    /// Creates a new config with only ws set
    pub fn set_ws(ws: impl Into<RpcModuleSelection>) -> Self {
        Self::default().with_ws(ws)
    }

    /// Creates a new config with only ipc set
    pub fn set_ipc(ipc: impl Into<RpcModuleSelection>) -> Self {
        Self::default().with_ipc(ipc)
    }

    /// Sets the [`RpcModuleSelection`] for the http transport.
    pub fn with_http(mut self, http: impl Into<RpcModuleSelection>) -> Self {
        self.http = Some(http.into());
        self
    }

    /// Sets the [`RpcModuleSelection`] for the ws transport.
    pub fn with_ws(mut self, ws: impl Into<RpcModuleSelection>) -> Self {
        self.ws = Some(ws.into());
        self
    }

    /// Sets the [`RpcModuleSelection`] for the http transport.
    pub fn with_ipc(mut self, ipc: impl Into<RpcModuleSelection>) -> Self {
        self.ipc = Some(ipc.into());
        self
    }

    /// Sets a custom [`RpcModuleConfig`] for the configured modules.
    pub const fn with_config(mut self, config: RpcModuleConfig) -> Self {
        self.config = Some(config);
        self
    }

    /// Get a mutable reference to the
    pub fn http_mut(&mut self) -> &mut Option<RpcModuleSelection> {
        &mut self.http
    }

    /// Get a mutable reference to the
    pub fn ws_mut(&mut self) -> &mut Option<RpcModuleSelection> {
        &mut self.ws
    }

    /// Get a mutable reference to the
    pub fn ipc_mut(&mut self) -> &mut Option<RpcModuleSelection> {
        &mut self.ipc
    }

    /// Get a mutable reference to the
    pub fn config_mut(&mut self) -> &mut Option<RpcModuleConfig> {
        &mut self.config
    }

    /// Returns true if no transports are configured
    pub const fn is_empty(&self) -> bool {
        self.http.is_none() && self.ws.is_none() && self.ipc.is_none()
    }

    /// Returns the [`RpcModuleSelection`] for the http transport
    pub const fn http(&self) -> Option<&RpcModuleSelection> {
        self.http.as_ref()
    }

    /// Returns the [`RpcModuleSelection`] for the ws transport
    pub const fn ws(&self) -> Option<&RpcModuleSelection> {
        self.ws.as_ref()
    }

    /// Returns the [`RpcModuleSelection`] for the ipc transport
    pub const fn ipc(&self) -> Option<&RpcModuleSelection> {
        self.ipc.as_ref()
    }

    /// Returns the [`RpcModuleConfig`] for the configured modules
    pub const fn config(&self) -> Option<&RpcModuleConfig> {
        self.config.as_ref()
    }

    /// Ensures that both http and ws are configured and that they are configured to use the same
    /// port.
    fn ensure_ws_http_identical(&self) -> Result<(), WsHttpSamePortError> {
        if RpcModuleSelection::are_identical(self.http.as_ref(), self.ws.as_ref()) {
            Ok(())
        } else {
            let http_modules =
                self.http.as_ref().map(RpcModuleSelection::to_selection).unwrap_or_default();
            let ws_modules =
                self.ws.as_ref().map(RpcModuleSelection::to_selection).unwrap_or_default();

            let http_not_ws = http_modules.difference(&ws_modules).copied().collect();
            let ws_not_http = ws_modules.difference(&http_modules).copied().collect();
            let overlap = http_modules.intersection(&ws_modules).copied().collect();

            Err(WsHttpSamePortError::ConflictingModules(Box::new(ConflictingModules {
                overlap,
                http_not_ws,
                ws_not_http,
            })))
        }
    }
}

/// Holds installed modules per transport type.
#[derive(Debug, Clone, Default)]
pub struct TransportRpcModules<Context = ()> {
    /// The original config
    config: TransportRpcModuleConfig,
    /// rpcs module for http
    http: Option<RpcModule<Context>>,
    /// rpcs module for ws
    ws: Option<RpcModule<Context>>,
    /// rpcs module for ipc
    ipc: Option<RpcModule<Context>>,
}

// === impl TransportRpcModules ===

impl TransportRpcModules {
    /// Returns the [`TransportRpcModuleConfig`] used to configure this instance.
    pub const fn module_config(&self) -> &TransportRpcModuleConfig {
        &self.config
    }

    /// Merge the given [Methods] in the configured http methods.
    ///
    /// Fails if any of the methods in other is present already.
    ///
    /// Returns [Ok(false)] if no http transport is configured.
    pub fn merge_http(&mut self, other: impl Into<Methods>) -> Result<bool, RegisterMethodError> {
        if let Some(ref mut http) = self.http {
            return http.merge(other.into()).map(|_| true);
        }
        Ok(false)
    }

    /// Merge the given [Methods] in the configured ws methods.
    ///
    /// Fails if any of the methods in other is present already.
    ///
    /// Returns [Ok(false)] if no ws transport is configured.
    pub fn merge_ws(&mut self, other: impl Into<Methods>) -> Result<bool, RegisterMethodError> {
        if let Some(ref mut ws) = self.ws {
            return ws.merge(other.into()).map(|_| true);
        }
        Ok(false)
    }

    /// Merge the given [Methods] in the configured ipc methods.
    ///
    /// Fails if any of the methods in other is present already.
    ///
    /// Returns [Ok(false)] if no ipc transport is configured.
    pub fn merge_ipc(&mut self, other: impl Into<Methods>) -> Result<bool, RegisterMethodError> {
        if let Some(ref mut ipc) = self.ipc {
            return ipc.merge(other.into()).map(|_| true);
        }
        Ok(false)
    }

    /// Merge the given [Methods] in all configured methods.
    ///
    /// Fails if any of the methods in other is present already.
    pub fn merge_configured(
        &mut self,
        other: impl Into<Methods>,
    ) -> Result<(), RegisterMethodError> {
        let other = other.into();
        self.merge_http(other.clone())?;
        self.merge_ws(other.clone())?;
        self.merge_ipc(other)?;
        Ok(())
    }

    /// Convenience function for starting a server
    pub async fn start_server(self, builder: RpcServerConfig) -> Result<RpcServerHandle, RpcError> {
        builder.start(self).await
    }
}

/// Container type for ws and http servers in all possible combinations.
#[derive(Default)]
struct WsHttpServer {
    /// The address of the http server
    http_local_addr: Option<SocketAddr>,
    /// The address of the ws server
    ws_local_addr: Option<SocketAddr>,
    /// Configured ws,http servers
    server: WsHttpServers,
    /// The jwt secret.
    jwt_secret: Option<JwtSecret>,
}

// Define the type alias with detailed type complexity
type WsHttpServerKind = Server<
    Stack<
        tower::util::Either<AuthLayer<JwtAuthValidator>, Identity>,
        Stack<tower::util::Either<CorsLayer, Identity>, Identity>,
    >,
    Stack<RpcRequestMetrics, Identity>,
>;

/// Enum for holding the http and ws servers in all possible combinations.
enum WsHttpServers {
    /// Both servers are on the same port
    SamePort(WsHttpServerKind),
    /// Servers are on different ports
    DifferentPort { http: Option<WsHttpServerKind>, ws: Option<WsHttpServerKind> },
}

// === impl WsHttpServers ===

impl WsHttpServers {
    /// Starts the servers and returns the handles (http, ws)
    async fn start(
        self,
        http_module: Option<RpcModule<()>>,
        ws_module: Option<RpcModule<()>>,
        config: &TransportRpcModuleConfig,
    ) -> Result<(Option<ServerHandle>, Option<ServerHandle>), RpcError> {
        let mut http_handle = None;
        let mut ws_handle = None;
        match self {
            Self::SamePort(server) => {
                // Make sure http and ws modules are identical, since we currently can't run
                // different modules on same server
                config.ensure_ws_http_identical()?;

                if let Some(module) = http_module.or(ws_module) {
                    let handle = server.start(module);
                    http_handle = Some(handle.clone());
                    ws_handle = Some(handle);
                }
            }
            Self::DifferentPort { http, ws } => {
                if let Some((server, module)) =
                    http.and_then(|server| http_module.map(|module| (server, module)))
                {
                    http_handle = Some(server.start(module));
                }
                if let Some((server, module)) =
                    ws.and_then(|server| ws_module.map(|module| (server, module)))
                {
                    ws_handle = Some(server.start(module));
                }
            }
        }

        Ok((http_handle, ws_handle))
    }
}

impl Default for WsHttpServers {
    fn default() -> Self {
        Self::DifferentPort { http: None, ws: None }
    }
}

/// Container type for each transport ie. http, ws, and ipc server
pub struct RpcServer {
    /// Configured ws,http servers
    ws_http: WsHttpServer,
    /// ipc server
    ipc: Option<IpcServer<Identity, Stack<RpcRequestMetrics, Identity>>>,
}

// === impl RpcServer ===

impl RpcServer {
    fn empty() -> Self {
        Self { ws_http: Default::default(), ipc: None }
    }

    /// Returns the [`SocketAddr`] of the http server if started.
    pub const fn http_local_addr(&self) -> Option<SocketAddr> {
        self.ws_http.http_local_addr
    }
    /// Return the `JwtSecret` of the server
    pub const fn jwt(&self) -> Option<JwtSecret> {
        self.ws_http.jwt_secret
    }

    /// Returns the [`SocketAddr`] of the ws server if started.
    pub const fn ws_local_addr(&self) -> Option<SocketAddr> {
        self.ws_http.ws_local_addr
    }

    /// Returns the endpoint of the ipc server if started.
    pub fn ipc_endpoint(&self) -> Option<String> {
        self.ipc.as_ref().map(|ipc| ipc.endpoint())
    }

    /// Starts the configured server by spawning the servers on the tokio runtime.
    ///
    /// This returns an [RpcServerHandle] that's connected to the server task(s) until the server is
    /// stopped or the [RpcServerHandle] is dropped.
    #[instrument(name = "start", skip_all, fields(http = ?self.http_local_addr(), ws = ?self.ws_local_addr(), ipc = ?self.ipc_endpoint()), target = "rpc", level = "TRACE")]
    pub async fn start(self, modules: TransportRpcModules) -> Result<RpcServerHandle, RpcError> {
        trace!(target: "rpc", "staring RPC server");
        let Self { ws_http, ipc: ipc_server } = self;
        let TransportRpcModules { config, http, ws, ipc } = modules;
        let mut handle = RpcServerHandle {
            http_local_addr: ws_http.http_local_addr,
            ws_local_addr: ws_http.ws_local_addr,
            http: None,
            ws: None,
            ipc_endpoint: None,
            ipc: None,
            jwt_secret: None,
        };

        let (http, ws) = ws_http.server.start(http, ws, &config).await?;
        handle.http = http;
        handle.ws = ws;

        if let Some((server, module)) =
            ipc_server.and_then(|server| ipc.map(|module| (server, module)))
        {
            handle.ipc_endpoint = Some(server.endpoint());
            handle.ipc = Some(server.start(module).await?);
        }

        Ok(handle)
    }
}

impl fmt::Debug for RpcServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RpcServer")
            .field("http", &self.ws_http.http_local_addr.is_some())
            .field("ws", &self.ws_http.ws_local_addr.is_some())
            .field("ipc", &self.ipc.is_some())
            .finish()
    }
}

/// A handle to the spawned servers.
///
/// When this type is dropped or [`RpcServerHandle::stop`] has been called the server will be
/// stopped.
#[derive(Clone, Debug)]
#[must_use = "Server stops if dropped"]
pub struct RpcServerHandle {
    /// The address of the http/ws server
    http_local_addr: Option<SocketAddr>,
    ws_local_addr: Option<SocketAddr>,
    http: Option<ServerHandle>,
    ws: Option<ServerHandle>,
    ipc_endpoint: Option<String>,
    ipc: Option<reth_ipc::server::ServerHandle>,
    jwt_secret: Option<JwtSecret>,
}

// === impl RpcServerHandle ===

impl RpcServerHandle {
    /// Configures the JWT secret for authentication.
    fn bearer_token(&self) -> Option<String> {
        self.jwt_secret.as_ref().map(|secret| {
            format!(
                "Bearer {}",
                secret
                    .encode(&Claims {
                        iat: (SystemTime::now().duration_since(UNIX_EPOCH).unwrap() +
                            Duration::from_secs(60))
                        .as_secs(),
                        exp: None,
                    })
                    .unwrap()
            )
        })
    }
    /// Returns the [`SocketAddr`] of the http server if started.
    pub const fn http_local_addr(&self) -> Option<SocketAddr> {
        self.http_local_addr
    }

    /// Returns the [`SocketAddr`] of the ws server if started.
    pub const fn ws_local_addr(&self) -> Option<SocketAddr> {
        self.ws_local_addr
    }

    /// Tell the server to stop without waiting for the server to stop.
    pub fn stop(self) -> Result<(), AlreadyStoppedError> {
        if let Some(handle) = self.http {
            handle.stop()?
        }

        if let Some(handle) = self.ws {
            handle.stop()?
        }

        if let Some(handle) = self.ipc {
            handle.stop()?
        }

        Ok(())
    }

    /// Returns the endpoint of the launched IPC server, if any
    pub fn ipc_endpoint(&self) -> Option<String> {
        self.ipc_endpoint.clone()
    }

    /// Returns the url to the http server
    pub fn http_url(&self) -> Option<String> {
        self.http_local_addr.map(|addr| format!("http://{addr}"))
    }

    /// Returns the url to the ws server
    pub fn ws_url(&self) -> Option<String> {
        self.ws_local_addr.map(|addr| format!("ws://{addr}"))
    }

    /// Returns a http client connected to the server.
    pub fn http_client(&self) -> Option<jsonrpsee::http_client::HttpClient> {
        let url = self.http_url()?;

        let client = if let Some(token) = self.bearer_token() {
            jsonrpsee::http_client::HttpClientBuilder::default()
                .set_headers(HeaderMap::from_iter([(AUTHORIZATION, token.parse().unwrap())]))
                .build(url)
        } else {
            jsonrpsee::http_client::HttpClientBuilder::default().build(url)
        };

        client.expect("failed to create http client").into()
    }

    /// Returns a ws client connected to the server.
    pub async fn ws_client(&self) -> Option<jsonrpsee::ws_client::WsClient> {
        let url = self.ws_url()?;
        let mut builder = jsonrpsee::ws_client::WsClientBuilder::default();

        if let Some(token) = self.bearer_token() {
            let headers = HeaderMap::from_iter([(AUTHORIZATION, token.parse().unwrap())]);
            builder = builder.set_headers(headers);
        }

        let client = builder.build(url).await.expect("failed to create ws client");
        Some(client)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_eth_call_bundle() {
        let selection = "eth-call-bundle".parse::<RethRpcModule>().unwrap();
        assert_eq!(selection, RethRpcModule::EthCallBundle);
        let selection = "eth_callBundle".parse::<RethRpcModule>().unwrap();
        assert_eq!(selection, RethRpcModule::EthCallBundle);
    }

    #[test]
    fn parse_eth_call_bundle_selection() {
        let selection = "eth,admin,debug,eth-call-bundle".parse::<RpcModuleSelection>().unwrap();
        assert_eq!(
            selection,
            RpcModuleSelection::Selection(
                [
                    RethRpcModule::Eth,
                    RethRpcModule::Admin,
                    RethRpcModule::Debug,
                    RethRpcModule::EthCallBundle,
                ]
                .into()
            )
        );
    }

    #[test]
    fn parse_rpc_module_selection() {
        let selection = "all".parse::<RpcModuleSelection>().unwrap();
        assert_eq!(selection, RpcModuleSelection::All);
    }

    #[test]
    fn parse_rpc_module_selection_none() {
        let selection = "none".parse::<RpcModuleSelection>().unwrap();
        assert_eq!(selection, RpcModuleSelection::Selection(Default::default()));
    }

    #[test]
    fn parse_rpc_unique_module_selection() {
        let selection = "eth,admin,eth,net".parse::<RpcModuleSelection>().unwrap();
        assert_eq!(
            selection,
            RpcModuleSelection::Selection(
                [RethRpcModule::Eth, RethRpcModule::Admin, RethRpcModule::Net,].into()
            )
        );
    }

    #[test]
    fn identical_selection() {
        assert!(RpcModuleSelection::are_identical(
            Some(&RpcModuleSelection::All),
            Some(&RpcModuleSelection::All),
        ));
        assert!(!RpcModuleSelection::are_identical(
            Some(&RpcModuleSelection::All),
            Some(&RpcModuleSelection::Standard),
        ));
        assert!(RpcModuleSelection::are_identical(
            Some(&RpcModuleSelection::Selection(RpcModuleSelection::Standard.to_selection())),
            Some(&RpcModuleSelection::Standard),
        ));
        assert!(RpcModuleSelection::are_identical(
            Some(&RpcModuleSelection::Selection([RethRpcModule::Eth].into())),
            Some(&RpcModuleSelection::Selection([RethRpcModule::Eth].into())),
        ));
        assert!(RpcModuleSelection::are_identical(
            None,
            Some(&RpcModuleSelection::Selection(Default::default())),
        ));
        assert!(RpcModuleSelection::are_identical(
            Some(&RpcModuleSelection::Selection(Default::default())),
            None,
        ));
        assert!(RpcModuleSelection::are_identical(None, None));
    }

    #[test]
    fn test_rpc_module_str() {
        macro_rules! assert_rpc_module {
            ($($s:expr => $v:expr,)*) => {
                $(
                    let val: RethRpcModule  = $s.parse().unwrap();
                    assert_eq!(val, $v);
                    assert_eq!(val.to_string().as_str(), $s);
                )*
            };
        }
        assert_rpc_module!
        (
                "admin" =>  RethRpcModule::Admin,
                "debug" =>  RethRpcModule::Debug,
                "eth" =>  RethRpcModule::Eth,
                "net" =>  RethRpcModule::Net,
                "trace" =>  RethRpcModule::Trace,
                "web3" =>  RethRpcModule::Web3,
                "rpc" => RethRpcModule::Rpc,
                "ots" => RethRpcModule::Ots,
                "reth" => RethRpcModule::Reth,
            );
    }

    #[test]
    fn test_default_selection() {
        let selection = RpcModuleSelection::Standard.to_selection();
        assert_eq!(selection, [RethRpcModule::Eth, RethRpcModule::Net, RethRpcModule::Web3].into())
    }

    #[test]
    fn test_create_rpc_module_config() {
        let selection = vec!["eth", "admin"];
        let config = RpcModuleSelection::try_from_selection(selection).unwrap();
        assert_eq!(
            config,
            RpcModuleSelection::Selection([RethRpcModule::Eth, RethRpcModule::Admin].into())
        );
    }

    #[test]
    fn test_configure_transport_config() {
        let config = TransportRpcModuleConfig::default()
            .with_http([RethRpcModule::Eth, RethRpcModule::Admin]);
        assert_eq!(
            config,
            TransportRpcModuleConfig {
                http: Some(RpcModuleSelection::Selection(
                    [RethRpcModule::Eth, RethRpcModule::Admin].into()
                )),
                ws: None,
                ipc: None,
                config: None,
            }
        )
    }

    #[test]
    fn test_configure_transport_config_none() {
        let config = TransportRpcModuleConfig::default().with_http(Vec::<RethRpcModule>::new());
        assert_eq!(
            config,
            TransportRpcModuleConfig {
                http: Some(RpcModuleSelection::Selection(Default::default())),
                ws: None,
                ipc: None,
                config: None,
            }
        )
    }
}

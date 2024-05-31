//! Configure reth RPC.
//!
//! This crate contains several builder and config types that allow to configure the selection of
//! [RethRpcModule] specific to transports (ws, http, ipc).
//!
//! The [RpcModuleBuilder] is the main entrypoint for configuring all reth modules. It takes
//! instances of components required to start the servers, such as provider impls, network and
//! transaction pool. [RpcModuleBuilder::build] returns a [TransportRpcModules] which contains the
//! transport specific config (what APIs are available via this transport).
//!
//! The [RpcServerConfig] is used to configure the [RpcServer] type which contains all transport
//! implementations (http server, ws server, ipc server). [RpcServer::start] requires the
//! [TransportRpcModules] so it can start the servers with the configured modules.
//!
//! # Examples
//!
//! Configure only an http server with a selection of [RethRpcModule]s
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
    metrics::RpcRequestMetrics, RpcModuleSelection::Selection,
};
use error::{ConflictingModules, RpcError, ServerKind};
use hyper::{header::AUTHORIZATION, HeaderMap};
pub use jsonrpsee::server::ServerBuilder;
use jsonrpsee::{
    core::RegisterMethodError,
    server::{AlreadyStoppedError, IdProvider, RpcServiceBuilder, Server, ServerHandle},
    Methods, RpcModule,
};
use reth_engine_primitives::EngineTypes;
use reth_evm::ConfigureEvm;
use reth_ipc::server::IpcServer;
pub use reth_ipc::server::{
    Builder as IpcServerBuilder, RpcServiceBuilder as IpcRpcServiceBuilder,
};
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
pub use reth_rpc_server_types::constants;
use reth_tasks::{
    pool::{BlockingTaskGuard, BlockingTaskPool},
    TaskSpawner, TokioTaskExecutor,
};
use reth_transaction_pool::{noop::NoopTransactionPool, TransactionPool};
use serde::{Deserialize, Serialize, Serializer};
use std::{
    collections::{HashMap, HashSet},
    fmt,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    str::FromStr,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use strum::{AsRefStr, EnumIter, IntoStaticStr, ParseError, VariantArray, VariantNames};
pub use tower::layer::util::{Identity, Stack};
use tower_http::cors::CorsLayer;
use tracing::{instrument, trace};

// re-export for convenience
pub use crate::eth::{EthConfig, EthHandlers};

/// Auth server utilities.
pub mod auth;

/// Eth utils
mod eth;

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

/// A builder type to configure the RPC module: See [RpcModule]
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

    /// Configure a [NoopTransactionPool] instance.
    ///
    /// Caution: This will configure a pool API that does absolutely nothing.
    /// This is only intended for allow easier setup of namespaces that depend on the [EthApi] which
    /// requires a [TransactionPool] implementation.
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

    /// Configure a [NoopNetwork] instance.
    ///
    /// Caution: This will configure a network API that does absolutely nothing.
    /// This is only intended for allow easier setup of namespaces that depend on the [EthApi] which
    /// requires a [NetworkInfo] implementation.
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

    /// Configure [TokioTaskExecutor] as the task executor to use for additional tasks.
    ///
    /// This will spawn additional tasks directly via `tokio::task::spawn`, See
    /// [TokioTaskExecutor].
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
    /// Configures all [RpcModule]s specific to the given [TransportRpcModuleConfig] which can be
    /// used to start the transport server(s).
    ///
    /// This behaves exactly as [RpcModuleBuilder::build] for the [TransportRpcModules], but also
    /// configures the auth (engine api) server, which exposes a subset of the `eth_` namespace.
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

    /// Converts the builder into a [RethModuleRegistry] which can be used to create all components.
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

    /// Configures all [RpcModule]s specific to the given [TransportRpcModuleConfig] which can be
    /// used to start the transport server(s).
    ///
    /// See also [RpcServer::start]
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
    /// Convenience method to create a new [RpcModuleConfigBuilder]
    pub fn builder() -> RpcModuleConfigBuilder {
        RpcModuleConfigBuilder::default()
    }
    /// Returns a new RPC module config given the eth namespace config
    pub const fn new(eth: EthConfig) -> Self {
        Self { eth }
    }
}

/// Configures [RpcModuleConfig]
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

    /// Consumes the type and creates the [RpcModuleConfig]
    pub fn build(self) -> RpcModuleConfig {
        let Self { eth } = self;
        RpcModuleConfig { eth: eth.unwrap_or_default() }
    }
}

/// Describes the modules that should be installed.
///
/// # Example
///
/// Create a [RpcModuleSelection] from a selection.
///
/// ```
/// use reth_rpc_builder::{RethRpcModule, RpcModuleSelection};
/// let config: RpcModuleSelection = vec![RethRpcModule::Eth].into();
/// ```
#[derive(Debug, Default, Clone, Eq, PartialEq)]
pub enum RpcModuleSelection {
    /// Use _all_ available modules.
    All,
    /// The default modules `eth`, `net`, `web3`
    #[default]
    Standard,
    /// Only use the configured modules.
    Selection(HashSet<RethRpcModule>),
}

// === impl RpcModuleSelection ===

impl RpcModuleSelection {
    /// The standard modules to instantiate by default `eth`, `net`, `web3`
    pub const STANDARD_MODULES: [RethRpcModule; 3] =
        [RethRpcModule::Eth, RethRpcModule::Net, RethRpcModule::Web3];

    /// Returns a selection of [RethRpcModule] with all [RethRpcModule::all_variants].
    pub fn all_modules() -> HashSet<RethRpcModule> {
        RethRpcModule::modules().into_iter().collect()
    }

    /// Returns the [RpcModuleSelection::STANDARD_MODULES] as a selection.
    pub fn standard_modules() -> HashSet<RethRpcModule> {
        HashSet::from(Self::STANDARD_MODULES)
    }

    /// All modules that are available by default on IPC.
    ///
    /// By default all modules are available on IPC.
    pub fn default_ipc_modules() -> HashSet<RethRpcModule> {
        Self::all_modules()
    }

    /// Creates a new _unique_ [RpcModuleSelection::Selection] from the given items.
    ///
    /// # Note
    ///
    /// This will dedupe the selection and remove duplicates while preserving the order.
    ///
    /// # Example
    ///
    /// Create a selection from the [RethRpcModule] string identifiers
    ///
    /// ```
    /// use reth_rpc_builder::{RethRpcModule, RpcModuleSelection};
    /// let selection = vec!["eth", "admin"];
    /// let config = RpcModuleSelection::try_from_selection(selection).unwrap();
    /// assert_eq!(config, RpcModuleSelection::from([RethRpcModule::Eth, RethRpcModule::Admin]));
    /// ```
    ///
    /// Create a unique selection from the [RethRpcModule] string identifiers
    ///
    /// ```
    /// use reth_rpc_builder::{RethRpcModule, RpcModuleSelection};
    /// let selection = vec!["eth", "admin", "eth", "admin"];
    /// let config = RpcModuleSelection::try_from_selection(selection).unwrap();
    /// assert_eq!(config, RpcModuleSelection::from([RethRpcModule::Eth, RethRpcModule::Admin]));
    /// ```
    pub fn try_from_selection<I, T>(selection: I) -> Result<Self, T::Error>
    where
        I: IntoIterator<Item = T>,
        T: TryInto<RethRpcModule>,
    {
        selection.into_iter().map(TryInto::try_into).collect()
    }

    /// Returns the number of modules in the selection
    pub fn len(&self) -> usize {
        match self {
            Self::All => RethRpcModule::variant_count(),
            Self::Standard => Self::STANDARD_MODULES.len(),
            Self::Selection(s) => s.len(),
        }
    }

    /// Returns true if no selection is configured
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Selection(sel) => sel.is_empty(),
            _ => false,
        }
    }

    /// Returns an iterator over all configured [RethRpcModule]
    pub fn iter_selection(&self) -> Box<dyn Iterator<Item = RethRpcModule> + '_> {
        match self {
            Self::All => Box::new(RethRpcModule::modules().into_iter()),
            Self::Standard => Box::new(Self::STANDARD_MODULES.iter().copied()),
            Self::Selection(s) => Box::new(s.iter().copied()),
        }
    }

    /// Clones the set of configured [RethRpcModule].
    pub fn to_selection(&self) -> HashSet<RethRpcModule> {
        match self {
            Self::All => Self::all_modules(),
            Self::Standard => Self::standard_modules(),
            Self::Selection(s) => s.clone(),
        }
    }

    /// Converts the selection into a [HashSet].
    pub fn into_selection(self) -> HashSet<RethRpcModule> {
        match self {
            Self::All => Self::all_modules(),
            Self::Standard => Self::standard_modules(),
            Self::Selection(s) => s,
        }
    }

    /// Returns true if both selections are identical.
    fn are_identical(http: Option<&Self>, ws: Option<&Self>) -> bool {
        match (http, ws) {
            // Shortcut for common case to avoid iterating later
            (Some(Self::All), Some(other)) | (Some(other), Some(Self::All)) => {
                other.len() == RethRpcModule::variant_count()
            }

            // If either side is disabled, then the other must be empty
            (Some(some), None) | (None, Some(some)) => some.is_empty(),

            (Some(http), Some(ws)) => http.to_selection() == ws.to_selection(),
            (None, None) => true,
        }
    }
}

impl From<&HashSet<RethRpcModule>> for RpcModuleSelection {
    fn from(s: &HashSet<RethRpcModule>) -> Self {
        Self::from(s.clone())
    }
}

impl From<HashSet<RethRpcModule>> for RpcModuleSelection {
    fn from(s: HashSet<RethRpcModule>) -> Self {
        Self::Selection(s)
    }
}

impl From<&[RethRpcModule]> for RpcModuleSelection {
    fn from(s: &[RethRpcModule]) -> Self {
        Self::Selection(s.iter().copied().collect())
    }
}

impl From<Vec<RethRpcModule>> for RpcModuleSelection {
    fn from(s: Vec<RethRpcModule>) -> Self {
        Self::Selection(s.into_iter().collect())
    }
}

impl<const N: usize> From<[RethRpcModule; N]> for RpcModuleSelection {
    fn from(s: [RethRpcModule; N]) -> Self {
        Self::Selection(s.iter().copied().collect())
    }
}

impl<'a> FromIterator<&'a RethRpcModule> for RpcModuleSelection {
    fn from_iter<I>(iter: I) -> Self
    where
        I: IntoIterator<Item = &'a RethRpcModule>,
    {
        iter.into_iter().copied().collect()
    }
}

impl FromIterator<RethRpcModule> for RpcModuleSelection {
    fn from_iter<I>(iter: I) -> Self
    where
        I: IntoIterator<Item = RethRpcModule>,
    {
        Self::Selection(iter.into_iter().collect())
    }
}

impl FromStr for RpcModuleSelection {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Ok(Selection(Default::default()))
        }
        let mut modules = s.split(',').map(str::trim).peekable();
        let first = modules.peek().copied().ok_or(ParseError::VariantNotFound)?;
        match first {
            "all" | "All" => Ok(Self::All),
            "none" | "None" => Ok(Selection(Default::default())),
            _ => Self::try_from_selection(modules),
        }
    }
}

impl fmt::Display for RpcModuleSelection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[{}]",
            self.iter_selection().map(|s| s.to_string()).collect::<Vec<_>>().join(", ")
        )
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

    /// Returns a merged RpcModule
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
    /// Instantiates AdminApi
    pub fn admin_api(&self) -> AdminApi<Network> {
        AdminApi::new(self.network.clone(), self.provider.chain_spec())
    }

    /// Instantiates Web3Api
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
    /// If called outside of the tokio runtime. See also [Self::eth_api]
    pub fn register_eth(&mut self) -> &mut Self {
        let eth_api = self.eth_api();
        self.modules.insert(RethRpcModule::Eth, eth_api.into_rpc().into());
        self
    }

    /// Register Otterscan Namespace
    ///
    /// # Panics
    ///
    /// If called outside of the tokio runtime. See also [Self::eth_api]
    pub fn register_ots(&mut self) -> &mut Self {
        let otterscan_api = self.otterscan_api();
        self.modules.insert(RethRpcModule::Ots, otterscan_api.into_rpc().into());
        self
    }

    /// Register Debug Namespace
    ///
    /// # Panics
    ///
    /// If called outside of the tokio runtime. See also [Self::eth_api]
    pub fn register_debug(&mut self) -> &mut Self {
        let debug_api = self.debug_api();
        self.modules.insert(RethRpcModule::Debug, debug_api.into_rpc().into());
        self
    }

    /// Register Trace Namespace
    ///
    /// # Panics
    ///
    /// If called outside of the tokio runtime. See also [Self::eth_api]
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
    /// See also [Self::eth_api]
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
    /// See also [Self::eth_api]
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

    /// Populates a new [RpcModule] based on the selected [RethRpcModule]s in the given
    /// [RpcModuleSelection]
    pub fn module_for(&mut self, config: &RpcModuleSelection) -> RpcModule<()> {
        let mut module = RpcModule::new(());
        let all_methods = self.reth_methods(config.iter_selection());
        for methods in all_methods {
            module.merge(methods).expect("No conflicts");
        }
        module
    }

    /// Returns the [Methods] for the given [RethRpcModule]
    ///
    /// If this is the first time the namespace is requested, a new instance of API implementation
    /// will be created.
    ///
    /// # Panics
    ///
    /// If called outside of the tokio runtime. See also [Self::eth_api]
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

    /// Returns the [EthStateCache] frontend
    ///
    /// This will spawn exactly one [EthStateCache] service if this is the first time the cache is
    /// requested.
    pub fn eth_cache(&mut self) -> EthStateCache {
        self.with_eth(|handlers| handlers.cache.clone())
    }

    /// Creates the [EthHandlers] type the first time this is called.
    ///
    /// This will spawn the required service tasks for [EthApi] for:
    ///   - [EthStateCache]
    ///   - [FeeHistoryCache]
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

    /// Returns the configured [EthHandlers] or creates it if it does not exist yet
    ///
    /// # Panics
    ///
    /// If called outside of the tokio runtime. See also [Self::eth_api]
    pub fn eth_handlers(&mut self) -> EthHandlers<Provider, Pool, Network, Events, EvmConfig> {
        self.with_eth(|handlers| handlers.clone())
    }

    /// Returns the configured [EthApi] or creates it if it does not exist yet
    ///
    /// Caution: This will spawn the necessary tasks required by the [EthApi]: [EthStateCache].
    ///
    /// # Panics
    ///
    /// If called outside of the tokio runtime.
    pub fn eth_api(&mut self) -> EthApi<Provider, Pool, Network, EvmConfig> {
        self.with_eth(|handlers| handlers.api.clone())
    }

    /// Instantiates TraceApi
    ///
    /// # Panics
    ///
    /// If called outside of the tokio runtime. See also [Self::eth_api]
    pub fn trace_api(&mut self) -> TraceApi<Provider, EthApi<Provider, Pool, Network, EvmConfig>> {
        let eth = self.eth_handlers();
        TraceApi::new(self.provider.clone(), eth.api, self.blocking_pool_guard.clone())
    }

    /// Instantiates [EthBundle] Api
    ///
    /// # Panics
    ///
    /// If called outside of the tokio runtime. See also [Self::eth_api]
    pub fn bundle_api(&mut self) -> EthBundle<EthApi<Provider, Pool, Network, EvmConfig>> {
        let eth_api = self.eth_api();
        EthBundle::new(eth_api, self.blocking_pool_guard.clone())
    }

    /// Instantiates OtterscanApi
    ///
    /// # Panics
    ///
    /// If called outside of the tokio runtime. See also [Self::eth_api]
    pub fn otterscan_api(&mut self) -> OtterscanApi<EthApi<Provider, Pool, Network, EvmConfig>> {
        let eth_api = self.eth_api();
        OtterscanApi::new(eth_api)
    }

    /// Instantiates DebugApi
    ///
    /// # Panics
    ///
    /// If called outside of the tokio runtime. See also [Self::eth_api]
    pub fn debug_api(&mut self) -> DebugApi<Provider, EthApi<Provider, Pool, Network, EvmConfig>> {
        let eth_api = self.eth_api();
        DebugApi::new(self.provider.clone(), eth_api, self.blocking_pool_guard.clone())
    }

    /// Instantiates NetApi
    ///
    /// # Panics
    ///
    /// If called outside of the tokio runtime. See also [Self::eth_api]
    pub fn net_api(&mut self) -> NetApi<Network, EthApi<Provider, Pool, Network, EvmConfig>> {
        let eth_api = self.eth_api();
        NetApi::new(self.network.clone(), eth_api)
    }

    /// Instantiates RethApi
    pub fn reth_api(&self) -> RethApi<Provider> {
        RethApi::new(self.provider.clone(), Box::new(self.executor.clone()))
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

    /// Sets the [RpcModuleSelection] for the http transport.
    pub fn with_http(mut self, http: impl Into<RpcModuleSelection>) -> Self {
        self.http = Some(http.into());
        self
    }

    /// Sets the [RpcModuleSelection] for the ws transport.
    pub fn with_ws(mut self, ws: impl Into<RpcModuleSelection>) -> Self {
        self.ws = Some(ws.into());
        self
    }

    /// Sets the [RpcModuleSelection] for the http transport.
    pub fn with_ipc(mut self, ipc: impl Into<RpcModuleSelection>) -> Self {
        self.ipc = Some(ipc.into());
        self
    }

    /// Sets a custom [RpcModuleConfig] for the configured modules.
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

    /// Returns the [RpcModuleSelection] for the http transport
    pub const fn http(&self) -> Option<&RpcModuleSelection> {
        self.http.as_ref()
    }

    /// Returns the [RpcModuleSelection] for the ws transport
    pub const fn ws(&self) -> Option<&RpcModuleSelection> {
        self.ws.as_ref()
    }

    /// Returns the [RpcModuleSelection] for the ipc transport
    pub const fn ipc(&self) -> Option<&RpcModuleSelection> {
        self.ipc.as_ref()
    }

    /// Returns the [RpcModuleConfig] for the configured modules
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
    /// Returns the [TransportRpcModuleConfig] used to configure this instance.
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
            return http.merge(other.into()).map(|_| true)
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
            return ws.merge(other.into()).map(|_| true)
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
            return ipc.merge(other.into()).map(|_| true)
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

/// A handle to the spawned servers.
///
/// When this type is dropped or [RpcServerHandle::stop] has been called the server will be stopped.
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
        assert_eq!(selection, Selection(Default::default()));
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

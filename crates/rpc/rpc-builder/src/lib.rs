#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]
#![allow(unused)]

//! Configure reth RPC
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
//! Configure only a http server with a selection of [RethRpcModule]s
//!
//! ```
//! use reth_network_api::{NetworkInfo, Peers};
//! use reth_provider::{BlockProvider, StateProviderFactory};
//! use reth_rpc_builder::{RethRpcModule, RpcModuleBuilder, RpcServerConfig, ServerBuilder, TransportRpcModuleConfig};
//! use reth_transaction_pool::TransactionPool;
//! pub async fn launch<Client, Pool, Network>(client: Client, pool: Pool, network: Network)
//! where
//!     Client: BlockProvider + StateProviderFactory + Clone + 'static,
//!     Pool: TransactionPool + Clone + 'static,
//!     Network: NetworkInfo + Peers + Clone + 'static,
//! {
//!     // configure the rpc module per transport
//!     let transports = TransportRpcModuleConfig::default().with_http(vec![
//!         RethRpcModule::Admin,
//!         RethRpcModule::Debug,
//!         RethRpcModule::Eth,
//!         RethRpcModule::Web3,
//!     ]);
//!     let transport_modules = RpcModuleBuilder::new(client, pool, network).build(transports);
//!     let handle = RpcServerConfig::default()
//!         .with_http(ServerBuilder::default())
//!         .start(transport_modules)
//!         .await
//!         .unwrap();
//! }
//! ```

pub use jsonrpsee::server::ServerBuilder;
use jsonrpsee::{
    core::{server::rpc_module::Methods, Error as RpcError},
    server::{Server, ServerHandle},
    RpcModule,
};
use reth_ipc::server::IpcServer;
pub use reth_ipc::server::{Builder as IpcServerBuilder, Endpoint};
use reth_network_api::{NetworkInfo, Peers};
use reth_provider::{BlockProvider, StateProviderFactory};
use reth_rpc::{AdminApi, DebugApi, EthApi, NetApi, TraceApi, Web3Api};
use reth_rpc_api::servers::*;
use reth_transaction_pool::TransactionPool;
use serde::{Deserialize, Serialize, Serializer};
use std::{
    collections::HashMap,
    fmt,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    str::FromStr,
};
use strum::{AsRefStr, EnumString, EnumVariantNames, ParseError, VariantNames};

/// The default port for the http/ws server
pub const DEFAULT_RPC_PORT: u16 = 8545;

/// The default IPC endpoint
#[cfg(windows)]
pub const DEFAULT_IPC_ENDPOINT: &str = r"\\.\pipe\reth.ipc";

/// The default IPC endpoint
#[cfg(not(windows))]
pub const DEFAULT_IPC_ENDPOINT: &str = "/tmp/reth.ipc";

/// Convenience function for starting a server in one step.
pub async fn launch<Client, Pool, Network>(
    client: Client,
    pool: Pool,
    network: Network,
    module_config: impl Into<TransportRpcModuleConfig>,
    server_config: impl Into<RpcServerConfig>,
) -> Result<RpcServerHandle, RpcError>
where
    Client: BlockProvider + StateProviderFactory + Clone + 'static,
    Pool: TransactionPool + Clone + 'static,
    Network: NetworkInfo + Peers + Clone + 'static,
{
    let module_config = module_config.into();
    let server_config = server_config.into();
    RpcModuleBuilder::new(client, pool, network)
        .build(module_config)
        .start_server(server_config)
        .await
}

/// A builder type to configure the RPC module: See [RpcModule]
///
/// This is the main entrypoint for up RPC servers.
#[derive(Debug)]
pub struct RpcModuleBuilder<Client, Pool, Network> {
    /// The Client type to when creating all rpc handlers
    client: Client,
    /// The Pool type to when creating all rpc handlers
    pool: Pool,
    /// The Network type to when creating all rpc handlers
    network: Network,
}

// === impl RpcBuilder ===

impl<Client, Pool, Network> RpcModuleBuilder<Client, Pool, Network> {
    /// Create a new instance of the builder
    pub fn new(client: Client, pool: Pool, network: Network) -> Self {
        Self { client, pool, network }
    }

    /// Configure the client instance.
    pub fn with_client<C>(self, client: C) -> RpcModuleBuilder<C, Pool, Network>
    where
        C: BlockProvider + StateProviderFactory + 'static,
    {
        let Self { pool, network, .. } = self;
        RpcModuleBuilder { client, network, pool }
    }

    /// Configure the transaction pool instance.
    pub fn with_pool<P>(self, pool: P) -> RpcModuleBuilder<Client, P, Network>
    where
        P: TransactionPool + 'static,
    {
        let Self { client, network, .. } = self;
        RpcModuleBuilder { client, network, pool }
    }

    /// Configure the network instance.
    pub fn with_network<N>(self, network: N) -> RpcModuleBuilder<Client, Pool, N>
    where
        N: NetworkInfo + Peers + 'static,
    {
        let Self { client, pool, .. } = self;
        RpcModuleBuilder { client, network, pool }
    }
}

impl<Client, Pool, Network> RpcModuleBuilder<Client, Pool, Network>
where
    Client: BlockProvider + StateProviderFactory + Clone + 'static,
    Pool: TransactionPool + Clone + 'static,
    Network: NetworkInfo + Peers + Clone + 'static,
{
    /// Configures all [RpcModule]s specific to the given [TransportRpcModuleConfig] which can be
    /// used to start the transport server(s).
    ///
    /// See also [RpcServer::start]
    pub fn build(self, module_config: TransportRpcModuleConfig) -> TransportRpcModules<()> {
        let mut modules = TransportRpcModules::default();

        let Self { client, pool, network } = self;

        let mut registry = RethModuleRegistry::new(client, pool, network);

        if !module_config.is_empty() {
            let TransportRpcModuleConfig { http, ws, ipc } = module_config;
            modules.http = registry.maybe_module(http.as_ref());
            modules.ws = registry.maybe_module(ws.as_ref());
            modules.ipc = registry.maybe_module(ipc.as_ref());
        }

        modules
    }
}

impl Default for RpcModuleBuilder<(), (), ()> {
    fn default() -> Self {
        RpcModuleBuilder::new((), (), ())
    }
}

/// Describes the modules that should be installed
///
/// # Example
///
/// Create a [RpcModuleConfig] from a selection.
///
/// ```
/// use reth_rpc_builder::{RethRpcModule, RpcModuleConfig};
/// let config: RpcModuleConfig = vec![RethRpcModule::Eth].into();
/// ```
#[derive(Debug, Default, Clone, Eq, PartialEq)]
pub enum RpcModuleConfig {
    /// Use _all_ available modules.
    All,
    /// The default modules `eth`, `net`, `web3`
    #[default]
    Standard,
    /// Only use the configured modules.
    Selection(Vec<RethRpcModule>),
}

// === impl RpcModuleConfig ===

impl RpcModuleConfig {
    /// The standard modules to instantiate by default `eth`, `net`, `web3`
    pub const STANDARD_MODULES: [RethRpcModule; 3] =
        [RethRpcModule::Eth, RethRpcModule::Net, RethRpcModule::Web3];

    /// Returns a selection of [RethRpcModule] with all [RethRpcModule::VARIANTS].
    pub fn all_modules() -> Vec<RethRpcModule> {
        RpcModuleConfig::try_from_selection(RethRpcModule::VARIANTS.iter().copied())
            .expect("valid selection")
            .into_selection()
    }

    /// Creates a new [RpcModuleConfig::Selection] from the given items.
    ///
    /// # Example
    ///
    /// Create a selection from the [RethRpcModule] string identifiers
    ///
    /// ```
    ///  use reth_rpc_builder::{RethRpcModule, RpcModuleConfig};
    /// let selection = vec!["eth", "admin"];
    /// let config = RpcModuleConfig::try_from_selection(selection).unwrap();
    /// assert_eq!(config, RpcModuleConfig::Selection(vec![RethRpcModule::Eth, RethRpcModule::Admin]));
    /// ```
    pub fn try_from_selection<I, T>(selection: I) -> Result<Self, T::Error>
    where
        I: IntoIterator<Item = T>,
        T: TryInto<RethRpcModule>,
    {
        let selection =
            selection.into_iter().map(TryInto::try_into).collect::<Result<Vec<_>, _>>()?;
        Ok(RpcModuleConfig::Selection(selection))
    }

    /// Creates a new [RpcModule] based on the configured reth modules.
    ///
    /// Note: This will always create new instance of the module handlers and is therefor only
    /// recommended for launching standalone transports. If multiple transports need to be
    /// configured it's recommended to use the [RpcModuleBuilder].
    pub fn standalone_module<Client, Pool, Network>(
        &self,
        client: Client,
        pool: Pool,
        network: Network,
    ) -> RpcModule<()>
    where
        Client: BlockProvider + StateProviderFactory + Clone + 'static,
        Pool: TransactionPool + Clone + 'static,
        Network: NetworkInfo + Peers + Clone + 'static,
    {
        let mut registry = RethModuleRegistry::new(client, pool, network);
        registry.module(self)
    }

    /// Returns an iterator over all configured [RethRpcModule]
    pub fn iter_selection(&self) -> Box<dyn Iterator<Item = RethRpcModule> + '_> {
        match self {
            RpcModuleConfig::All => Box::new(Self::all_modules().into_iter()),
            RpcModuleConfig::Standard => Box::new(Self::STANDARD_MODULES.iter().copied()),
            RpcModuleConfig::Selection(s) => Box::new(s.iter().copied()),
        }
    }

    /// Returns the list of configured [RethRpcModule]
    pub fn into_selection(self) -> Vec<RethRpcModule> {
        match self {
            RpcModuleConfig::All => Self::all_modules(),
            RpcModuleConfig::Selection(s) => s,
            RpcModuleConfig::Standard => Self::STANDARD_MODULES.to_vec(),
        }
    }
}

impl<I, T> From<I> for RpcModuleConfig
where
    I: IntoIterator<Item = T>,
    T: Into<RethRpcModule>,
{
    fn from(value: I) -> Self {
        RpcModuleConfig::Selection(value.into_iter().map(Into::into).collect())
    }
}

impl FromStr for RpcModuleConfig {
    type Err = ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let modules = s.split(',');

        RpcModuleConfig::try_from_selection(modules)
    }
}

/// Represents RPC modules that are supported by reth
#[derive(
    Debug, Clone, Copy, Eq, PartialEq, Hash, AsRefStr, EnumVariantNames, EnumString, Deserialize,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "kebab-case")]
pub enum RethRpcModule {
    /// `admin_` module
    Admin,
    /// `debug_` module
    Debug,
    /// `eth_` module
    Eth,
    /// `net_` module
    Net,
    /// `trace_` module
    Trace,
    /// `web3_` module
    Web3,
}

impl fmt::Display for RethRpcModule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(self.as_ref())
    }
}

impl Serialize for RethRpcModule {
    fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        s.serialize_str(self.as_ref())
    }
}

/// A Helper type the holds instances of the configured modules.
pub struct RethModuleRegistry<Client, Pool, Network> {
    client: Client,
    pool: Pool,
    network: Network,
    /// Holds a clone of the actual [EthApi] namespace impl since this can be required by other
    /// namespaces
    eth_api: Option<EthApi<Client, Pool, Network>>,
    /// Contains the [Methods] of a module
    modules: HashMap<RethRpcModule, Methods>,
}

// === impl RethModuleRegistry ===

impl<Client, Pool, Network> RethModuleRegistry<Client, Pool, Network>
where
    Client: BlockProvider + StateProviderFactory + Clone + 'static,
    Pool: TransactionPool + Clone + 'static,
    Network: NetworkInfo + Peers + Clone + 'static,
{
    /// Creates a new, empty instance
    pub fn new(client: Client, pool: Pool, network: Network) -> Self {
        Self { client, pool, network, eth_api: None, modules: Default::default() }
    }

    /// Helper function to create a [RpcModule] if it's not `None`
    fn maybe_module(&mut self, config: Option<&RpcModuleConfig>) -> Option<RpcModule<()>> {
        let config = config?;
        let module = self.module(config);
        Some(module)
    }

    /// Populates a new [RpcModule] based on the selected [RethRpcModule]s in the given
    /// [RpcModuleConfig]
    pub fn module(&mut self, config: &RpcModuleConfig) -> RpcModule<()> {
        let mut module = RpcModule::new(());
        for reth_module in config.iter_selection() {
            let methods = self.reth_methods(reth_module);
            module.merge(methods).expect("No conflicts");
        }
        module
    }

    /// Returns the [Methods] for the given [RethRpcModule]
    ///
    /// If this is the first time the namespace is requested, a new instance of API implementation
    /// will be created.
    pub fn reth_methods(&mut self, namespace: RethRpcModule) -> Methods {
        if let Some(methods) = self.modules.get(&namespace).cloned() {
            return methods
        }
        let methods: Methods = match namespace {
            RethRpcModule::Admin => AdminApi::new(self.network.clone()).into_rpc().into(),
            RethRpcModule::Debug => DebugApi::new().into_rpc().into(),
            RethRpcModule::Eth => self.eth_api().into_rpc().into(),
            RethRpcModule::Net => {
                let eth_api = self.eth_api();
                NetApi::new(self.network.clone(), eth_api).into_rpc().into()
            }
            RethRpcModule::Trace => TraceApi::new().into_rpc().into(),
            RethRpcModule::Web3 => Web3Api::new(self.network.clone()).into_rpc().into(),
        };
        self.modules.insert(namespace, methods.clone());

        methods
    }

    /// Returns the configured [EthApi] or creates it if it does not exist yet
    fn eth_api(&mut self) -> EthApi<Client, Pool, Network> {
        self.eth_api
            .get_or_insert_with(|| {
                EthApi::new(self.client.clone(), self.pool.clone(), self.network.clone())
            })
            .clone()
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
/// Once the [RpcModule] is built via [RpcModuleBuilder] the servers can be started, See also
/// [ServerBuilder::build] and [Server::start](jsonrpsee::server::Server::start).
#[derive(Default)]
pub struct RpcServerConfig {
    /// Configs for JSON-RPC Http.
    http_server_config: Option<ServerBuilder>,
    /// Configs for WS server
    ws_server_config: Option<ServerBuilder>,
    /// Address where to bind the http and ws server to
    http_ws_addr: Option<SocketAddr>,
    /// Configs for JSON-RPC IPC server
    ipc_server_config: Option<IpcServerBuilder>,
    /// The Endpoint where to launch the ipc server
    ipc_endpoint: Option<Endpoint>,
}

/// === impl RpcServerConfig ===

impl RpcServerConfig {
    /// Creates a new config with only http set
    pub fn http(config: ServerBuilder) -> Self {
        Self::default().with_http(config)
    }

    /// Creates a new config with only ws set
    pub fn ws(config: ServerBuilder) -> Self {
        Self::default().with_ws(config)
    }

    /// Creates a new config with only ipc set
    pub fn ipc(config: IpcServerBuilder) -> Self {
        Self::default().with_ipc(config)
    }

    /// Configures the http server
    pub fn with_http(mut self, config: ServerBuilder) -> Self {
        self.http_server_config = Some(config.http_only());
        self
    }

    /// Configures the ws server
    pub fn with_ws(mut self, config: ServerBuilder) -> Self {
        self.ws_server_config = Some(config.ws_only());
        self
    }

    /// Configures the [SocketAddr] of the http/ws server
    ///
    /// Default is [Ipv4Addr::UNSPECIFIED] and [DEFAULT_RPC_PORT]
    pub fn with_address(mut self, addr: SocketAddr) -> Self {
        self.http_ws_addr = Some(addr);
        self
    }

    /// Configures the ipc server
    pub fn with_ipc(mut self, mut config: IpcServerBuilder) -> Self {
        self.ipc_server_config = Some(config);
        self
    }

    /// Configures the endpoint of the ipc server
    ///
    /// Default is [DEFAULT_IPC_ENDPOINT]
    pub fn with_ipc_endpoint(mut self, path: impl Into<String>) -> Self {
        self.ipc_endpoint = Some(Endpoint::new(path.into()));
        self
    }

    /// Convenience function to do [RpcServerConfig::build] and [RpcServer::start] in one step
    pub async fn start(
        self,
        modules: TransportRpcModules<()>,
    ) -> Result<RpcServerHandle, RpcError> {
        self.build().await?.start(modules).await
    }

    /// Finalize the configuration of the server(s).
    ///
    /// This consumes the builder and returns a server.
    ///
    /// Note: The server ist not started and does nothing unless polled, See also [RpcServer::start]
    pub async fn build(self) -> Result<RpcServer, RpcError> {
        let socket_addr = self
            .http_ws_addr
            .unwrap_or(SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, DEFAULT_RPC_PORT)));

        let mut local_addr = None;

        let http_server = if let Some(builder) = self.http_server_config {
            let server = builder.build(socket_addr).await?;
            if local_addr.is_none() {
                local_addr = server.local_addr().ok();
            }
            Some(server)
        } else {
            None
        };

        let ws_server = if let Some(builder) = self.ws_server_config {
            let server = builder.build(socket_addr).await.unwrap();
            if local_addr.is_none() {
                local_addr = server.local_addr().ok();
            }
            Some(server)
        } else {
            None
        };

        let ipc_server = if let Some(builder) = self.ipc_server_config {
            let ipc_path = self
                .ipc_endpoint
                .unwrap_or_else(|| Endpoint::new(DEFAULT_IPC_ENDPOINT.to_string()));
            let server = builder.build(ipc_path.path())?;
            Some(server)
        } else {
            None
        };

        Ok(RpcServer { local_addr, http: http_server, ws: ws_server, ipc: ipc_server })
    }
}

/// Holds modules to be installed per transport type
///
/// # Example
///
/// Configure an http transport only
///
/// ```
/// use reth_rpc_builder::{RethRpcModule, TransportRpcModuleConfig};
///  let config = TransportRpcModuleConfig::default()
///       .with_http([RethRpcModule::Eth, RethRpcModule::Admin]);
/// ```
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct TransportRpcModuleConfig {
    /// http module configuration
    http: Option<RpcModuleConfig>,
    /// ws module configuration
    ws: Option<RpcModuleConfig>,
    /// ipc module configuration
    ipc: Option<RpcModuleConfig>,
}

// === impl TransportRpcModuleConfig ===

impl TransportRpcModuleConfig {
    /// Creates a new config with only http set
    pub fn http(http: impl Into<RpcModuleConfig>) -> Self {
        Self::default().with_http(http)
    }

    /// Creates a new config with only ws set
    pub fn ws(ws: impl Into<RpcModuleConfig>) -> Self {
        Self::default().with_ws(ws)
    }

    /// Creates a new config with only ipc set
    pub fn ipc(ipc: impl Into<RpcModuleConfig>) -> Self {
        Self::default().with_ipc(ipc)
    }

    /// Sets the [RpcModuleConfig] for the http transport.
    pub fn with_http(mut self, http: impl Into<RpcModuleConfig>) -> Self {
        self.http = Some(http.into());
        self
    }

    /// Sets the [RpcModuleConfig] for the ws transport.
    pub fn with_ws(mut self, ws: impl Into<RpcModuleConfig>) -> Self {
        self.ws = Some(ws.into());
        self
    }

    /// Sets the [RpcModuleConfig] for the http transport.
    pub fn with_ipc(mut self, ipc: impl Into<RpcModuleConfig>) -> Self {
        self.ipc = Some(ipc.into());
        self
    }

    /// Returns true if no transports are configured
    pub fn is_empty(&self) -> bool {
        self.http.is_none() && self.ws.is_none() && self.ipc.is_none()
    }
}

/// Holds installed modules per transport type.
#[derive(Debug, Default)]
pub struct TransportRpcModules<Context> {
    /// rpcs module for http
    http: Option<RpcModule<Context>>,
    /// rpcs module for ws
    ws: Option<RpcModule<Context>>,
    /// rpcs module for ipc
    ipc: Option<RpcModule<Context>>,
}

// === impl TransportRpcModules ===

impl TransportRpcModules<()> {
    /// Convenience function for starting a server
    pub async fn start_server(self, builder: RpcServerConfig) -> Result<RpcServerHandle, RpcError> {
        builder.start(self).await
    }
}

/// Container type for each transport ie. http, ws, and ipc server
pub struct RpcServer {
    /// The address of the http/ws server
    local_addr: Option<SocketAddr>,
    /// http server
    http: Option<Server>,
    /// ws server
    ws: Option<Server>,
    /// ipc server
    ipc: Option<IpcServer>,
}

// === impl RpcServer ===

impl RpcServer {
    /// Returns the [`SocketAddr`] of the http/ws server if started.
    fn local_addr(&self) -> Option<SocketAddr> {
        self.local_addr
    }

    /// Starts the configured server by spawning the servers on the tokio runtime.
    ///
    /// This returns an [RpcServerHandle] that's connected to the server task(s) until the server is
    /// stopped or the [RpcServerHandle] is dropped.
    pub async fn start(
        self,
        modules: TransportRpcModules<()>,
    ) -> Result<RpcServerHandle, RpcError> {
        let TransportRpcModules { http, ws, ipc } = modules;
        let mut handle =
            RpcServerHandle { local_addr: self.local_addr, http: None, ws: None, ipc: None };

        // Start all servers
        if let Some((server, module)) =
            self.http.and_then(|server| http.map(|module| (server, module)))
        {
            handle.http = Some(server.start(module)?);
        }

        if let Some((server, module)) = self.ws.and_then(|server| ws.map(|module| (server, module)))
        {
            handle.ws = Some(server.start(module)?);
        }

        if let Some((server, module)) =
            self.ipc.and_then(|server| ipc.map(|module| (server, module)))
        {
            handle.ipc = Some(server.start(module).await?);
        }

        Ok(handle)
    }
}

impl std::fmt::Debug for RpcServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RpcServer")
            .field("http", &self.http.is_some())
            .field("ws", &self.ws.is_some())
            .field("ipc", &self.ipc.is_some())
            .finish()
    }
}

/// A handle to the spawned servers.
///
/// When this type is dropped or [RpcServerHandle::stop] has been called the server will be stopped.
#[derive(Clone)]
#[must_use = "Server stop if dropped"]
pub struct RpcServerHandle {
    /// The address of the http/ws server
    local_addr: Option<SocketAddr>,
    http: Option<ServerHandle>,
    ws: Option<ServerHandle>,
    ipc: Option<ServerHandle>,
}

// === impl RpcServerHandle ===

impl RpcServerHandle {
    /// Returns the [`SocketAddr`] of the http/ws server if started.
    fn local_addr(&self) -> Option<SocketAddr> {
        self.local_addr
    }

    /// Tell the server to stop without waiting for the server to stop.
    pub fn stop(self) -> Result<(), RpcError> {
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

    /// Returns the url to the http server
    pub fn http_url(&self) -> Option<String> {
        self.local_addr.map(|addr| format!("http://{addr}"))
    }

    /// Returns the url to the ws server
    pub fn ws_url(&self) -> Option<String> {
        self.local_addr.map(|addr| format!("ws://{addr}"))
    }

    /// Returns a http client connected to the server.
    pub fn http_client(&self) -> Option<jsonrpsee::http_client::HttpClient> {
        let url = self.http_url()?;
        let client = jsonrpsee::http_client::HttpClientBuilder::default()
            .build(url)
            .expect("Failed to create http client");
        Some(client)
    }

    /// Returns a ws client connected to the server.
    pub async fn ws_client(&self) -> Option<jsonrpsee::ws_client::WsClient> {
        let url = self.ws_url()?;
        let client = jsonrpsee::ws_client::WsClientBuilder::default()
            .build(url)
            .await
            .expect("Failed to create ws client");
        Some(client)
    }
}

impl std::fmt::Debug for RpcServerHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RpcServerHandle")
            .field("http", &self.http.is_some())
            .field("ws", &self.ws.is_some())
            .field("ipc", &self.ipc.is_some())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            );
    }

    #[test]
    fn test_default_selection() {
        let selection = RpcModuleConfig::Standard.into_selection();
        assert_eq!(selection, vec![RethRpcModule::Eth, RethRpcModule::Net, RethRpcModule::Web3,])
    }

    #[test]
    fn test_create_rpc_module_config() {
        let selection = vec!["eth", "admin"];
        let config = RpcModuleConfig::try_from_selection(selection).unwrap();
        assert_eq!(
            config,
            RpcModuleConfig::Selection(vec![RethRpcModule::Eth, RethRpcModule::Admin])
        );
    }

    #[test]
    fn test_configure_transport_config() {
        let config = TransportRpcModuleConfig::default()
            .with_http([RethRpcModule::Eth, RethRpcModule::Admin]);
        assert_eq!(
            config,
            TransportRpcModuleConfig {
                http: Some(RpcModuleConfig::Selection(vec![
                    RethRpcModule::Eth,
                    RethRpcModule::Admin
                ])),
                ws: None,
                ipc: None,
            }
        )
    }
}

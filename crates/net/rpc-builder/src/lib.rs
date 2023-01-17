#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]

//! Configure reth RPC

use jsonrpsee::{server::ServerBuilder, RpcModule};
use reth_ipc::server::{Builder as IpcServerBuilder, Endpoint};
use reth_network_api::{NetworkInfo, PeersInfo};
use reth_provider::{BlockProvider, StateProviderFactory};
use reth_transaction_pool::TransactionPool;
use serde::{Deserialize, Serialize, Serializer};
use std::{fmt, net::SocketAddr};
use strum::{AsRefStr, EnumString, EnumVariantNames};

/// A builder type to configure the RPC module: See [RpcModule]
///
/// This is the main entrypoint for up RPC servers.
///
/// ```
///  use reth_rpc_builder::{RethRpcModule, RpcModuleBuilder, RpcModuleConfig};
/// let builder = RpcModuleBuilder::default()
///     .with_config(RpcModuleConfig::Selection(vec![RethRpcModule::Eth]));
/// ```
#[derive(Debug)]
pub struct RpcModuleBuilder<Client, Pool, Network> {
    /// The Client type to when creating all rpc handlers
    client: Client,
    /// The Pool type to when creating all rpc handlers
    pool: Pool,
    /// The Network type to when creating all rpc handlers
    network: Network,
    /// What modules to configure
    config: RpcModuleConfig,
}

// === impl RpcBuilder ===

impl<Client, Pool, Network> RpcModuleBuilder<Client, Pool, Network> {
    /// Create a new instance of the builder
    pub fn new(client: Client, pool: Pool, network: Network) -> Self {
        Self { client, pool, network, config: Default::default() }
    }

    /// Configures what RPC modules should be installed
    pub fn with_config(mut self, config: RpcModuleConfig) -> Self {
        self.config = config;
        self
    }

    /// Configure the client instance.
    pub fn with_client<C>(self, client: C) -> RpcModuleBuilder<C, Pool, Network>
    where
        C: BlockProvider + StateProviderFactory + 'static,
    {
        let Self { pool, config, network, .. } = self;
        RpcModuleBuilder { client, config, network, pool }
    }

    /// Configure the transaction pool instance.
    pub fn with_pool<P>(self, pool: P) -> RpcModuleBuilder<Client, P, Network>
    where
        P: TransactionPool + 'static,
    {
        let Self { client, config, network, .. } = self;
        RpcModuleBuilder { client, config, network, pool }
    }

    /// Configure the network instance.
    pub fn with_network<N>(self, network: N) -> RpcModuleBuilder<Client, Pool, N>
    where
        N: NetworkInfo + PeersInfo + 'static,
    {
        let Self { client, config, pool, .. } = self;
        RpcModuleBuilder { client, config, network, pool }
    }
}

impl<Client, Pool, Network> RpcModuleBuilder<Client, Pool, Network>
where
    Client: BlockProvider + StateProviderFactory + 'static,
    Pool: TransactionPool + 'static,
    Network: NetworkInfo + PeersInfo + 'static,
{
    /// Configures the [RpcModule] which can be used to start the server(s).
    pub fn build(self) -> RpcModule<()> {
        let Self { client: _, pool: _, network: _, config: _ } = self;
        let _io = RpcModule::new(());

        todo!("merge all handlers")
    }
}

impl Default for RpcModuleBuilder<(), (), ()> {
    fn default() -> Self {
        RpcModuleBuilder::new((), (), ())
    }
}

/// Describes the modules that should be installed
#[derive(Debug, Default)]
pub enum RpcModuleConfig {
    /// Use all available modules.
    #[default]
    All,
    /// Only use the configured modules.
    Selection(Vec<RethRpcModule>),
}

/// Represents RPC modules that are supported by reth
#[derive(
    Debug, Clone, Copy, Eq, PartialEq, AsRefStr, EnumVariantNames, EnumString, Deserialize,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "kebab-case")]
pub enum RethRpcModule {
    /// `admin_` module
    Admin,
    /// `eth_` module
    Eth,
    /// `web3_` module
    Web3,
    /// `trace_` module
    Trace,
    /// `debug_` module
    Debug,
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
pub struct RpcServerBuilder {
    /// Configs for JSON-RPC Http and WS server
    pub http_ws_server_config: Option<ServerBuilder>,
    /// Address where to bind the http and ws server to
    pub http_ws_addr: Option<SocketAddr>,
    /// Configs for JSON-RPC IPC server
    pub ipc_server_config: Option<IpcServerBuilder>,
    /// The Endpoint where to launch the ipc server
    pub ipc_server_path: Option<Endpoint>,
}

#![warn(missing_docs, unreachable_pub)]
#![deny(unused_must_use, rust_2018_idioms)]
#![doc(test(
    no_crate_inject,
    attr(deny(warnings, rust_2018_idioms), allow(dead_code, unused_variables))
))]
#![allow(unused)]

//! Configure reth RPC

use jsonrpsee::{
    core::{server::rpc_module::Methods, Error as RpcError},
    server::{Server, ServerBuilder, ServerHandle},
    RpcModule,
};
use reth_ipc::server::{Builder as IpcServerBuilder, Endpoint, IpcServer};
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
    ///
    /// See also [RpcServer::start]
    pub fn build(self, _servers: TransportRpcModuleConfig) -> TransportRpcModules<()> {
        let Self { client: _, pool: _, network: _, config: _ } = self;

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
    /// Configs for JSON-RPC Http.
    pub http_server_config: Option<ServerBuilder>,
    /// Configs for WS server
    pub ws_server_config: Option<ServerBuilder>,
    /// Address where to bind the http and ws server to
    pub http_ws_addr: Option<SocketAddr>,
    /// Configs for JSON-RPC IPC server
    pub ipc_server_config: Option<IpcServerBuilder>,
    /// The Endpoint where to launch the ipc server
    pub ipc_server_path: Option<Endpoint>,
}

/// === impl RpcServerBuilder ===

impl RpcServerBuilder {
    /// Finalize the configuration of the server(s).
    ///
    /// This consumes the builder and returns a server.
    ///
    /// Note: The server ist not started and does nothing unless polled, See also
    pub async fn build(self) -> Result<RpcServer, RpcError> {
        let socket_addr = self.http_ws_addr.unwrap_or("127.0.0.1:8545".parse().unwrap());
        /// Todo: default path for ipc
        let ips_path = self.ipc_server_path.unwrap(); // default path undecided

        let http_server = if let Some(builder) = self.http_server_config {
            let server = builder.build(socket_addr).await?;
            Some(server)
        } else {
            None
        };

        let ws_server = if let Some(builder) = self.ws_server_config {
            let server = builder.build(socket_addr).await?;
            Some(server)
        } else {
            None
        };

        let ipc_server = if let Some(builder) = self.ipc_server_config {
            let server = builder.build(ips_path.path())?;
            Some(server)
        } else {
            None
        };

        Ok(RpcServer { http: http_server, ws: ws_server, ipc: ipc_server })
    }
}

/// Holds modules to be installed per transport type
pub struct TransportRpcModuleConfig {
    /// http module configuration
    http: Option<RpcModuleConfig>,
    /// ws module configuration
    ws: Option<RpcModuleConfig>,
    /// ipc module configuration
    ipc: Option<RpcModuleConfig>,
}

/// Holds installed modules per transport type
pub struct TransportRpcModules<Context> {
    /// rpcs module for http
    http: Option<RpcModule<Context>>,
    /// rpcs module for ws
    ws: Option<RpcModule<Context>>,
    /// rpcs module for ipc
    ipc: Option<RpcModule<Context>>,
}

/// Container type for each trandport ie. http, ws, and ipc server
pub struct RpcServer {
    /// http server
    http: Option<Server>,
    /// ws server
    ws: Option<Server>,
    /// ipc server
    ipc: Option<IpcServer>,
}

// === impl RpcServer ===

impl RpcServer {
    /// Starts the configured server by spawning the servers on the tokio runtime.
    ///
    /// This returns an [RpcServerHandle] that's connected to the server task(s) until the server is
    /// stopped or the [RpcServerHandle] is dropped.
    pub async fn start(self, _methods: impl Into<Methods>) -> Result<RpcServerHandle, RpcError> {
        todo!()
    }
}

/// A handle to the spawned servers.
///
/// When stop has been called the server will be stopped.
#[derive(Debug, Clone)]
pub struct RpcServerHandle {
    http: Option<ServerHandle>,
    ws: Option<ServerHandle>,
    ipc: Option<ServerHandle>,
}

// === impl RpcServerHandle ===

impl RpcServerHandle {
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
}

//! Support for integrating customizations into the CLI.

use crate::runner::CliContext;
use clap::Args;
use reth_network_api::{NetworkInfo, Peers};
use reth_provider::{
    BlockReaderIdExt, CanonStateSubscriptions, ChainSpecProvider, ChangeSetReader, EvmEnvProvider,
    HeaderProvider, StateProviderFactory,
};
use reth_rpc::JwtSecret;
use reth_rpc_builder::{
    auth::AuthServerHandle, error::RpcError, EthConfig, RpcServerHandle, TransportRpcModules,
};
use reth_rpc_engine_api::EngineApiServer;
use reth_tasks::TaskSpawner;
use reth_transaction_pool::TransactionPool;
use std::any::Any;

/// A trait that allows bootstrapping the node.
pub trait RethNodeBuilder {
    /// A type that knows how to create reth's RPC server.
    type Rpc: RethRpcServerBuilder;

    /// Runs the node.
    fn execute(self, ctx: CliContext) -> eyre::Result<()>;
}

/// A trait that allows customizing the RPC server creation
///
/// TODO impl this for server args
#[async_trait::async_trait]
pub trait RethRpcServerBuilder {
    /// Configures and launches _all_ servers.
    #[allow(clippy::too_many_arguments)]
    async fn start_servers<Provider, Pool, Network, Tasks, Events, Engine>(
        &self,
        provider: Provider,
        pool: Pool,
        network: Network,
        executor: Tasks,
        events: Events,
        engine_api: Engine,
        jwt_secret: JwtSecret,
    ) -> eyre::Result<ServerHandles>
    where
        Provider: BlockReaderIdExt
            + HeaderProvider
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
        Engine: EngineApiServer;
}

/// A handle to the servers that are running.
#[must_use = "The servers will be stopped if this is dropped."]
pub struct ServerHandles {
    /// The RPC server handle.
    pub rpc_server_handle: Option<RpcServerHandle>,
    /// The RPC server to the engine/auth server
    pub auth_server_handle: Option<AuthServerHandle>,
    /// Any additional data that needs to be kept alive for the lifetime of the server.
    pub keep_alive: Option<Box<dyn Any + Send>>,
}

impl ServerHandles {
    /// Creates a new server handle using the given RPC server handle and auth handle.
    pub fn new(rpc_server_handle: RpcServerHandle, auth_server_handle: AuthServerHandle) -> Self {
        Self {
            rpc_server_handle: Some(rpc_server_handle),
            auth_server_handle: Some(auth_server_handle),
            keep_alive: None,
        }
    }
}

/// A trait that provides configured RPC server.
///
/// This provides all basic config values for the RPC server and is implemented by the
/// [RpcServerArgs](crate::args::RpcServerArgs) type.
pub trait RpcServerConfig {
    /// Returns whether ipc is enabled.
    fn is_ipc_enabled(&self) -> bool;

    /// The configured ethereum RPC settings.
    fn eth_config(&self) -> EthConfig;
}

/// A trait that allows further customization of the RPC server via CLI.
pub trait RpcServerArgsExt: clap::Args {
    /// Allows for registering additional RPC modules for the transports.
    ///
    /// This is expected to call the merge functions of [TransportRpcModules], for example
    /// [TransportRpcModules::merge_configured]
    fn extend_rpc_modules<Conf>(&self, modules: &mut TransportRpcModules<()>) -> eyre::Result<()>
    where
        Conf: RpcServerConfig;
}

//! Components that are used by the node command.

use reth_network::{NetworkEvents, NetworkProtocols};
use reth_network_api::{NetworkInfo, Peers};
use reth_primitives::ChainSpec;
use reth_provider::{
    AccountReader, BlockReaderIdExt, CanonStateSubscriptions, ChainSpecProvider, ChangeSetReader,
    EvmEnvProvider, StateProviderFactory,
};
use reth_rpc_builder::{
    auth::AuthServerHandle, RethModuleRegistry, RpcServerHandle, TransportRpcModules,
};
use reth_tasks::TaskSpawner;
use reth_transaction_pool::TransactionPool;
use std::sync::Arc;

/// Helper trait to unify all provider traits for simplicity.
pub trait FullProvider:
    BlockReaderIdExt
    + AccountReader
    + StateProviderFactory
    + EvmEnvProvider
    + ChainSpecProvider
    + ChangeSetReader
    + Clone
    + Unpin
    + 'static
{
}

impl<T> FullProvider for T where
    T: BlockReaderIdExt
        + AccountReader
        + StateProviderFactory
        + EvmEnvProvider
        + ChainSpecProvider
        + ChangeSetReader
        + Clone
        + Unpin
        + 'static
{
}

/// The trait that is implemented for the Node command.
pub trait RethNodeComponents: Clone + Send + Sync + 'static {
    /// The Provider type that is provided by the not itself
    type Provider: FullProvider;
    /// The transaction pool type
    type Pool: TransactionPool + Clone + Unpin + 'static;
    /// The network type used to communicate with p2p.
    type Network: NetworkInfo + Peers + NetworkProtocols + NetworkEvents + Clone + 'static;
    /// The events type used to create subscriptions.
    type Events: CanonStateSubscriptions + Clone + 'static;
    /// The type that is used to spawn tasks.
    type Tasks: TaskSpawner + Clone + Unpin + 'static;

    /// Returns the instance of the provider
    fn provider(&self) -> Self::Provider;

    /// Returns the instance of the task executor.
    fn task_executor(&self) -> Self::Tasks;

    /// Returns the instance of the transaction pool.
    fn pool(&self) -> Self::Pool;

    /// Returns the instance of the network API.
    fn network(&self) -> Self::Network;

    /// Returns the instance of the events subscription handler.
    fn events(&self) -> Self::Events;

    /// Helper function to return the chain spec.
    fn chain_spec(&self) -> Arc<ChainSpec> {
        self.provider().chain_spec()
    }
}

/// Helper container to encapsulate [RethModuleRegistry] and [TransportRpcModules].
///
/// This can be used to access installed modules, or create commonly used handlers like
/// [reth_rpc::EthApi], and ultimately merge additional rpc handler into the configured transport
/// modules [TransportRpcModules].
#[derive(Debug)]
#[allow(clippy::type_complexity)]
pub struct RethRpcComponents<'a, Reth: RethNodeComponents> {
    /// A Helper type the holds instances of the configured modules.
    ///
    /// This provides easy access to rpc handlers, such as [RethModuleRegistry::eth_api].
    pub registry: &'a mut RethModuleRegistry<
        Reth::Provider,
        Reth::Pool,
        Reth::Network,
        Reth::Tasks,
        Reth::Events,
    >,
    /// Holds installed modules per transport type.
    ///
    /// This can be used to merge additional modules into the configured transports (http, ipc,
    /// ws). See [TransportRpcModules::merge_configured]
    pub modules: &'a mut TransportRpcModules,
}

/// A Generic implementation of the RethNodeComponents trait.
///
/// Represents components required for the Reth node.
#[derive(Clone, Debug)]
pub struct RethNodeComponentsImpl<Provider, Pool, Network, Events, Tasks> {
    /// Represents the provider instance.
    pub provider: Provider,
    /// Represents the transaction pool instance.
    pub pool: Pool,
    /// Represents the network instance used for communication.
    pub network: Network,
    /// Represents the task executor instance.
    pub task_executor: Tasks,
    /// Represents the events subscription handler instance.
    pub events: Events,
}

impl<Provider, Pool, Network, Events, Tasks> RethNodeComponents
    for RethNodeComponentsImpl<Provider, Pool, Network, Events, Tasks>
where
    Provider: FullProvider + Clone + 'static,
    Tasks: TaskSpawner + Clone + Unpin + 'static,
    Pool: TransactionPool + Clone + Unpin + 'static,
    Network: NetworkInfo + Peers + NetworkProtocols + NetworkEvents + Clone + 'static,
    Events: CanonStateSubscriptions + Clone + 'static,
{
    type Provider = Provider;
    type Pool = Pool;
    type Network = Network;
    type Events = Events;
    type Tasks = Tasks;

    fn provider(&self) -> Self::Provider {
        self.provider.clone()
    }

    fn task_executor(&self) -> Self::Tasks {
        self.task_executor.clone()
    }

    fn pool(&self) -> Self::Pool {
        self.pool.clone()
    }

    fn network(&self) -> Self::Network {
        self.network.clone()
    }

    fn events(&self) -> Self::Events {
        self.events.clone()
    }
}

/// Contains the handles to the spawned RPC servers.
///
/// This can be used to access the endpoints of the servers.
///
/// # Example
///
/// ```rust
/// use reth::{cli::components::RethRpcServerHandles, rpc::api::EthApiClient};
/// # async fn t(handles: RethRpcServerHandles) {
/// let client = handles.rpc.http_client().expect("http server not started");
/// let block_number = client.block_number().await.unwrap();
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct RethRpcServerHandles {
    /// The regular RPC server handle.
    pub rpc: RpcServerHandle,
    /// The handle to the auth server (engine API)
    pub auth: AuthServerHandle,
}

//! Components that are used by the node command.

use reth_network_api::{NetworkInfo, Peers};
use reth_primitives::ChainSpec;
use reth_provider::{
    AccountReader, BlockReaderIdExt, CanonStateSubscriptions, ChainSpecProvider, ChangeSetReader,
    EvmEnvProvider, StateProviderFactory,
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
pub trait RethNodeComponents {
    /// The Provider type that is provided by the not itself
    type Provider: FullProvider;
    /// The transaction pool type
    type Pool: TransactionPool + Clone + Unpin + 'static;
    /// The network type used to communicate with p2p.
    type Network: NetworkInfo + Peers + Clone + 'static;
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

/// A Generic implementation of the RethNodeComponents trait.
#[derive(Clone, Debug)]
#[allow(missing_docs)]
pub struct RethNodeComponentsImpl<Provider, Pool, Network, Events, Tasks> {
    pub provider: Provider,
    pub pool: Pool,
    pub network: Network,
    pub task_executor: Tasks,
    pub events: Events,
}

impl<Provider, Pool, Network, Events, Tasks> RethNodeComponents
    for RethNodeComponentsImpl<Provider, Pool, Network, Events, Tasks>
where
    Provider: FullProvider + Clone + 'static,
    Tasks: TaskSpawner + Clone + Unpin + 'static,
    Pool: TransactionPool + Clone + Unpin + 'static,
    Network: NetworkInfo + Peers + Clone + 'static,
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

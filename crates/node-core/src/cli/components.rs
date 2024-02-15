//! Components that are used by the node command.

use reth_db::database::Database;
use reth_network::{NetworkEvents, NetworkProtocols};
use reth_network_api::{NetworkInfo, Peers};
use reth_node_api::ConfigureEvmEnv;
use reth_primitives::ChainSpec;
use reth_provider::{
    AccountReader, BlockReaderIdExt, CanonStateSubscriptions, ChainSpecProvider, ChangeSetReader,
    DatabaseProviderFactory, EvmEnvProvider, StateProviderFactory,
};
use reth_rpc_builder::{
    auth::{AuthRpcModule, AuthServerHandle},
    RethModuleRegistry, RpcServerHandle, TransportRpcModules,
};
use reth_tasks::TaskSpawner;
use reth_transaction_pool::TransactionPool;
use std::{marker::PhantomData, sync::Arc};

/// Helper trait to unify all provider traits for simplicity.
pub trait FullProvider<DB: Database>:
    DatabaseProviderFactory<DB>
    + BlockReaderIdExt
    + AccountReader
    + StateProviderFactory
    + EvmEnvProvider
    + ChainSpecProvider
    + ChangeSetReader
    + CanonStateSubscriptions
    + Clone
    + Unpin
    + 'static
{
}

impl<T, DB: Database> FullProvider<DB> for T where
    T: DatabaseProviderFactory<DB>
        + BlockReaderIdExt
        + AccountReader
        + StateProviderFactory
        + EvmEnvProvider
        + ChainSpecProvider
        + ChangeSetReader
        + CanonStateSubscriptions
        + Clone
        + Unpin
        + 'static
{
}
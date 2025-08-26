//! Traits for configuring a node.

use crate::PayloadTypes;
use reth_basic_payload_builder::PayloadBuilder;
use reth_consensus::{ConsensusError, FullConsensus};
use reth_db_api::{database_metrics::DatabaseMetrics, Database};
use reth_evm::ConfigureEvm;
use reth_network_api::FullNetwork;
use reth_node_types::{NodeTypes, NodeTypesWithDBAdapter, TxTy};
use reth_payload_builder::PayloadBuilderHandle;
use reth_provider::FullProvider;
use reth_tasks::TaskExecutor;
use reth_transaction_pool::{PoolTransaction, TransactionPool};
use std::{fmt::Debug, marker::PhantomData};

/// A helper trait that is downstream of the [`NodeTypes`] trait and adds stateful
/// components to the node.
///
/// Its types are configured by node internally and are not intended to be user configurable.
pub trait FullNodeTypes: Clone + Debug + Send + Sync + Unpin + 'static {
    /// Node's types with the database.
    type Types: NodeTypes;
    /// Underlying database type used by the node to store and retrieve data.
    type DB: Database + DatabaseMetrics + Clone + Unpin + 'static;
    /// The provider type used to interact with the node.
    type Provider: FullProvider<NodeTypesWithDBAdapter<Self::Types, Self::DB>>;
}

/// An adapter type that adds the builtin provider type to the user configured node types.
#[derive(Clone, Debug)]
pub struct FullNodeTypesAdapter<Types, DB, Provider>(PhantomData<(Types, DB, Provider)>);

impl<Types, DB, Provider> FullNodeTypes for FullNodeTypesAdapter<Types, DB, Provider>
where
    Types: NodeTypes,
    DB: Database + DatabaseMetrics + Clone + Unpin + 'static,
    Provider: FullProvider<NodeTypesWithDBAdapter<Types, DB>>,
{
    type Types = Types;
    type DB = DB;
    type Provider = Provider;
}

/// Helper trait to bound [`PayloadBuilder`] to the node's engine types.
pub trait PayloadBuilderFor<N: NodeTypes>:
    PayloadBuilder<
    Attributes = <N::Payload as PayloadTypes>::PayloadBuilderAttributes,
    BuiltPayload = <N::Payload as PayloadTypes>::BuiltPayload,
>
{
}

impl<T, N: NodeTypes> PayloadBuilderFor<N> for T where
    T: PayloadBuilder<
        Attributes = <N::Payload as PayloadTypes>::PayloadBuilderAttributes,
        BuiltPayload = <N::Payload as PayloadTypes>::BuiltPayload,
    >
{
}

/// Encapsulates all types and components of the node.
pub trait FullNodeComponents: FullNodeTypes + Clone + 'static {
    /// The transaction pool of the node.
    type Pool: TransactionPool<Transaction: PoolTransaction<Consensus = TxTy<Self::Types>>> + Unpin;

    /// The node's EVM configuration, defining settings for the Ethereum Virtual Machine.
    type Evm: ConfigureEvm<Primitives = <Self::Types as NodeTypes>::Primitives>;

    /// The consensus type of the node.
    type Consensus: FullConsensus<<Self::Types as NodeTypes>::Primitives, Error = ConsensusError>
        + Clone
        + Unpin
        + 'static;

    /// Network API.
    type Network: FullNetwork;

    /// Returns the transaction pool of the node.
    fn pool(&self) -> &Self::Pool;

    /// Returns the node's evm config.
    fn evm_config(&self) -> &Self::Evm;

    /// Returns the node's consensus type.
    fn consensus(&self) -> &Self::Consensus;

    /// Returns the handle to the network
    fn network(&self) -> &Self::Network;

    /// Returns the handle to the payload builder service handling payload building requests from
    /// the engine.
    fn payload_builder_handle(&self) -> &PayloadBuilderHandle<<Self::Types as NodeTypes>::Payload>;

    /// Returns the provider of the node.
    fn provider(&self) -> &Self::Provider;

    /// Returns an executor handle to spawn tasks.
    ///
    /// This can be used to spawn critical, blocking tasks or register tasks that should be
    /// terminated gracefully. See also [`TaskSpawner`](reth_tasks::TaskSpawner).
    fn task_executor(&self) -> &TaskExecutor;
}

//! Traits for configuring a node.

use std::marker::PhantomData;

use reth_chainspec::{ChainSpec, EthChainSpec};
use reth_db_api::{
    database::Database,
    database_metrics::{DatabaseMetadata, DatabaseMetrics},
};
use reth_evm::execute::BlockExecutorProvider;
use reth_network_api::FullNetwork;
use reth_payload_builder::PayloadBuilderHandle;
use reth_provider::FullProvider;
use reth_rpc_eth_api::EthApiTypes;
use reth_tasks::TaskExecutor;
use reth_transaction_pool::TransactionPool;

use crate::{primitives::NodePrimitives, ConfigureEvm, EngineTypes};

/// The type that configures the essential types of an ethereum like node.
///
/// This includes the primitive types of a node, the engine API types for communication with the
/// consensus layer.
///
/// This trait is intended to be stateless and only define the types of the node.
pub trait NodeTypes: Send + Sync + Unpin + 'static {
    /// The node's primitive types, defining basic operations and structures.
    type Primitives: NodePrimitives;
    /// The node's engine types, defining the interaction with the consensus engine.
    type Engine: EngineTypes;
    /// The type used for configuration of the EVM.
    type ChainSpec: EthChainSpec;
}

/// A [`NodeTypes`] type builder
#[derive(Default, Debug)]
pub struct AnyNodeTypes<P = (), E = (), C = ()>(PhantomData<P>, PhantomData<E>, PhantomData<C>);

impl<P, E, C> AnyNodeTypes<P, E, C> {
    /// Sets the `Primitives` associated type.
    pub const fn primitives<T>(self) -> AnyNodeTypes<T, E, C> {
        AnyNodeTypes::<T, E, C>(PhantomData::<T>, PhantomData::<E>, PhantomData::<C>)
    }

    /// Sets the `Engine` associated type.
    pub const fn engine<T>(self) -> AnyNodeTypes<P, T, C> {
        AnyNodeTypes::<P, T, C>(PhantomData::<P>, PhantomData::<T>, PhantomData::<C>)
    }
}

impl<P, E, C> NodeTypes for AnyNodeTypes<P, E, C>
where
    P: NodePrimitives + Send + Sync + Unpin + 'static,
    E: EngineTypes + Send + Sync + Unpin,
    C: EthChainSpec,
{
    type Primitives = P;

    type Engine = E;

    type ChainSpec = C;
}

/// A helper trait that is downstream of the [`NodeTypes`] trait and adds stateful components to the
/// node.
///
/// Its types are configured by node internally and are not intended to be user configurable.
pub trait FullNodeTypes: NodeTypes<ChainSpec = ChainSpec> + 'static {
    /// Underlying database type used by the node to store and retrieve data.
    type DB: Database + DatabaseMetrics + DatabaseMetadata + Clone + Unpin + 'static;
    /// The provider type used to interact with the node.
    type Provider: FullProvider<Self::DB, Self::ChainSpec>;
}

/// An adapter type that adds the builtin provider type to the user configured node types.
#[derive(Debug)]
pub struct FullNodeTypesAdapter<Types, DB, Provider> {
    /// An instance of the user configured node types.
    pub types: PhantomData<Types>,
    /// The database type used by the node.
    pub db: PhantomData<DB>,
    /// The provider type used by the node.
    pub provider: PhantomData<Provider>,
}

impl<Types, DB, Provider> FullNodeTypesAdapter<Types, DB, Provider> {
    /// Create a new adapter with the configured types.
    pub fn new() -> Self {
        Self { types: Default::default(), db: Default::default(), provider: Default::default() }
    }
}

impl<Types, DB, Provider> Default for FullNodeTypesAdapter<Types, DB, Provider> {
    fn default() -> Self {
        Self::new()
    }
}

impl<Types, DB, Provider> Clone for FullNodeTypesAdapter<Types, DB, Provider> {
    fn clone(&self) -> Self {
        Self { types: self.types, db: self.db, provider: self.provider }
    }
}

impl<Types, DB, Provider> NodeTypes for FullNodeTypesAdapter<Types, DB, Provider>
where
    Types: NodeTypes,
    DB: Send + Sync + Unpin + 'static,
    Provider: Send + Sync + Unpin + 'static,
{
    type Primitives = Types::Primitives;
    type Engine = Types::Engine;
    type ChainSpec = Types::ChainSpec;
}

impl<Types, DB, Provider> FullNodeTypes for FullNodeTypesAdapter<Types, DB, Provider>
where
    Types: NodeTypes<ChainSpec = ChainSpec>,
    Provider: FullProvider<DB, Types::ChainSpec>,
    DB: Database + DatabaseMetrics + DatabaseMetadata + Clone + Unpin + 'static,
{
    type DB = DB;
    type Provider = Provider;
}

/// Encapsulates all types and components of the node.
pub trait FullNodeComponents: FullNodeTypes + Clone + 'static {
    /// The transaction pool of the node.
    type Pool: TransactionPool + Unpin;

    /// The node's EVM configuration, defining settings for the Ethereum Virtual Machine.
    type Evm: ConfigureEvm;

    /// The type that knows how to execute blocks.
    type Executor: BlockExecutorProvider;

    /// Network API.
    type Network: FullNetwork;

    /// Returns the transaction pool of the node.
    fn pool(&self) -> &Self::Pool;

    /// Returns the node's evm config.
    fn evm_config(&self) -> &Self::Evm;

    /// Returns the node's executor type.
    fn block_executor(&self) -> &Self::Executor;

    /// Returns the provider of the node.
    fn provider(&self) -> &Self::Provider;

    /// Returns the handle to the network
    fn network(&self) -> &Self::Network;

    /// Returns the handle to the payload builder service.
    fn payload_builder(&self) -> &PayloadBuilderHandle<Self::Engine>;

    /// Returns handle to runtime.
    fn task_executor(&self) -> &TaskExecutor;
}

/// Customizable node add-on types.
pub trait NodeAddOns<N: FullNodeComponents>: Send + Sync + Unpin + Clone + 'static {
    /// The core `eth` namespace API type to install on the RPC server (see
    /// `reth_rpc_eth_api::EthApiServer`).
    type EthApi: EthApiTypes + Send + Clone;
}

impl<N: FullNodeComponents> NodeAddOns<N> for () {
    type EthApi = ();
}

/// Returns the builder for type.
pub trait BuilderProvider<N: FullNodeComponents>: Send {
    /// Context required to build type.
    type Ctx<'a>;

    /// Returns builder for type.
    #[allow(clippy::type_complexity)]
    fn builder() -> Box<dyn for<'a> Fn(Self::Ctx<'a>) -> Self + Send>;
}

impl<N: FullNodeComponents> BuilderProvider<N> for () {
    type Ctx<'a> = ();

    fn builder() -> Box<dyn for<'a> Fn(Self::Ctx<'a>) -> Self + Send> {
        Box::new(noop_builder)
    }
}

const fn noop_builder(_: ()) {}

//! Traits for configuring a node.

use std::{fmt, marker::PhantomData, ops};

use reth_db_api::{
    database::Database,
    database_metrics::{DatabaseMetadata, DatabaseMetrics},
};
use reth_evm::execute::BlockExecutorProvider;
use reth_network::{FullClient, NetworkHandle};
use reth_payload_builder::PayloadBuilderHandle;
use reth_provider::FullProvider;
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
    type EngineTypes: EngineTypes;
}

impl<T> NodeTypes for T
where
    T: ops::Deref + Send + Sync + Unpin + 'static,
    <T as ops::Deref>::Target: NodeTypes,
{
    type Primitives = <<T as ops::Deref>::Target as NodeTypes>::Primitives;
    type EngineTypes = <<T as ops::Deref>::Target as NodeTypes>::EngineTypes;
}

/// A [`NodeTypes`] type builder
#[derive(Default, Debug)]
pub struct AnyNodeTypes<P = (), E = ()>(PhantomData<P>, PhantomData<E>);

impl<P, E> AnyNodeTypes<P, E> {
    /// Sets the `Primitives` associated type.
    pub const fn primitives<T>(self) -> AnyNodeTypes<T, E> {
        AnyNodeTypes::<T, E>(PhantomData::<T>, PhantomData::<E>)
    }

    /// Sets the `Engine` associated type.
    pub const fn engine<T>(self) -> AnyNodeTypes<P, T> {
        AnyNodeTypes::<P, T>(PhantomData::<P>, PhantomData::<T>)
    }
}

impl<P, E> NodeTypes for AnyNodeTypes<P, E>
where
    P: NodePrimitives + Send + Sync + Unpin + 'static,
    E: EngineTypes + Send + Sync + Unpin + 'static,
{
    type Primitives = P;

    type EngineTypes = E;
}

/// A helper trait that is downstream of the [`NodeTypes`] trait and adds stateful components to the
/// node.
///
/// Its types are configured by node internally and are not intended to be user configurable.
pub trait FullNodeTypes: NodeTypes + 'static {
    /// Underlying database type used by the node to store and retrieve data.
    type DB: Database + DatabaseMetrics + DatabaseMetadata + Clone + Unpin + 'static;
    /// The provider type used to interact with the node.
    type Provider: FullProvider<Self::DB>;
}

impl<T> FullNodeTypes for T
where
    T: NodeTypes + ops::Deref,
    <T as ops::Deref>::Target: FullNodeTypes,
{
    type DB = <<T as ops::Deref>::Target as FullNodeTypes>::DB;
    type Provider = <<T as ops::Deref>::Target as FullNodeTypes>::Provider;
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
    type EngineTypes = Types::EngineTypes;
}

impl<Types, DB, Provider> FullNodeTypes for FullNodeTypesAdapter<Types, DB, Provider>
where
    Types: NodeTypes,
    Provider: FullProvider<DB>,
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

    /// Returns the transaction pool of the node.
    fn pool(&self) -> &Self::Pool;

    /// Returns the node's evm config.
    fn evm_config(&self) -> &Self::Evm;

    /// Returns the node's executor type.
    fn block_executor(&self) -> &Self::Executor;

    /// Returns the provider of the node.
    fn provider(&self) -> &Self::Provider;

    /// Returns the handle to the network
    fn network(&self) -> &NetworkHandle;

    /// Returns the handle to the payload builder service.
    fn payload_builder(&self) -> &PayloadBuilderHandle<Self::EngineTypes>;

    /// Returns the task executor.
    fn task_executor(&self) -> &TaskExecutor;
}

/// An intermediary type for `FullNodeComponentsExt`, that isn't `Clone`.
pub trait FullNodeComponentsExt: FullNodeTypes + Clone + 'static {
    type Core: FullNodeComponents;
    type Tree;
    type Pipeline: PipelineComponent;
    type Engine: EngineComponent<Self::Core> + 'static;
    type Rpc: RpcComponent<Self::Core> + 'static;

    fn from_core(core: Self::Core) -> Self;

    fn core(&self) -> &Self::Core;

    /// Returns reference to blockchain tree component, if installed.
    fn tree(&self) -> Option<&Self::Tree>;

    /// Returns reference to pipeline component, if installed.
    fn pipeline(&self) -> Option<&Self::Pipeline>;

    /// Returns reference to consensus engine component, if installed.
    fn engine(&self) -> Option<&Self::Engine>;

    /// Returns reference to RPC component, if installed.
    fn rpc(&self) -> Option<&Self::Rpc>;

    fn into_core(self) -> Self::Core;
}

impl<T> FullNodeComponents for T
where
    T: FullNodeComponentsExt + ops::Deref,
    T::Target: FullNodeComponents<Provider = T::Provider, EngineTypes = T::EngineTypes>,
{
    type Pool = <T::Target as FullNodeComponents>::Pool;
    type Evm = <T::Target as FullNodeComponents>::Evm;
    type Executor = <T::Target as FullNodeComponents>::Executor;

    fn pool(&self) -> &Self::Pool {
        self.deref().pool()
    }

    fn evm_config(&self) -> &Self::Evm {
        self.deref().evm_config()
    }

    fn block_executor(&self) -> &Self::Executor {
        self.deref().block_executor()
    }

    fn provider(&self) -> &Self::Provider {
        self.deref().provider()
    }

    fn network(&self) -> &NetworkHandle {
        self.deref().network()
    }

    fn payload_builder(&self) -> &PayloadBuilderHandle<Self::EngineTypes> {
        self.deref().payload_builder()
    }

    fn task_executor(&self) -> &TaskExecutor {
        self.deref().task_executor()
    }
}

pub trait TreeComponent: Send + Sync + Unpin + Clone + 'static {
    // ..
}

pub trait PipelineComponent: Send + Sync + Unpin + Clone + 'static {
    type Client: FullClient + Send + Sync + Clone;
    // ..
}

pub trait EngineComponent<N: FullNodeComponents>: Send + Unpin + Clone + 'static {
    type Engine: Send + 'static;
    type Handle: Send + Sync + Unpin + 'static;
    type ShutdownRx: Send + Unpin + Default + 'static;

    fn engine(&self) -> &Self::Engine;

    fn handle(&self) -> &Self::Handle;

    fn shutdown_rx_mut(&mut self) -> &mut Self::ShutdownRx;
}

impl<N: FullNodeComponents> EngineComponent<N> for Option<()> {
    type Engine = Self;
    type Handle = Self;
    type ShutdownRx = Self;

    fn engine(&self) -> &Self::Engine {
        self
    }

    fn handle(&self) -> &Self::Handle {
        self
    }

    fn shutdown_rx_mut(&mut self) -> &mut Self::ShutdownRx {
        self
    }
}

pub trait RpcComponent<N: FullNodeComponents>: Send + Sync + Unpin + Clone + 'static {
    type ServerHandles: Send + Sync + Unpin + fmt::Debug + Clone + 'static;
    type Registry: Send + Unpin + fmt::Debug + Clone + 'static;

    fn handles(&self) -> &Self::ServerHandles;

    fn registry(&self) -> &Self::Registry;
}

impl<N: FullNodeComponents> RpcComponent<N> for Option<()> {
    type ServerHandles = Self;
    type Registry = Self;

    fn handles(&self) -> &Self::ServerHandles {
        self
    }

    fn registry(&self) -> &Self::Registry {
        self
    }
}

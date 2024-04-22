//! Traits for configuring a node

use crate::{primitives::NodePrimitives, ConfigureEvm, EngineTypes};
use reth_db::database::Database;
use reth_evm::execute::BlockExecutorProvider;
use reth_network::NetworkHandle;
use reth_payload_builder::PayloadBuilderHandle;
use reth_provider::FullProvider;
use reth_tasks::TaskExecutor;
use reth_transaction_pool::TransactionPool;
use std::marker::PhantomData;

/// The type that configures the essential types of an ethereum like node.
///
/// This includes the primitive types of a node, the engine API types for communication with the
/// consensus layer, and the EVM configuration type for setting up the Ethereum Virtual Machine.
pub trait NodeTypes: Send + Sync + 'static {
    /// The node's primitive types, defining basic operations and structures.
    type Primitives: NodePrimitives;
    /// The node's engine types, defining the interaction with the consensus engine.
    type Engine: EngineTypes;
    /// The node's EVM configuration, defining settings for the Ethereum Virtual Machine.
    type Evm: ConfigureEvm;
    /// Provides instances of executors to execute blocks, for individual blocks and batches of
    /// blocks (historical sync).
    type BlockExecutor: BlockExecutorProvider;

    /// Returns the node's evm config.
    fn evm_config(&self) -> Self::Evm;

    /// Returns the node's executor.
    ///
    /// NOTE:
    ///
    /// This function is temporary and will be moved in the future.
    /// It is currently part of this crate because it is required by the provider implementation.
    ///
    /// Blocked by <https://github.com/paradigmxyz/reth/issues/7154>
    ///
    /// TODO add prune modes etc as arguments
    fn block_executor(&self) -> Self::BlockExecutor;
}

/// A helper type that is downstream of the [NodeTypes] trait and adds stateful components to the
/// node.
pub trait FullNodeTypes: NodeTypes + 'static {
    /// Underlying database type.
    type DB: Database + Clone + 'static;
    /// The provider type used to interact with the node.
    type Provider: FullProvider<Self::DB>;
}

/// An adapter type that adds the builtin provider type to the user configured node types.
#[derive(Debug)]
pub struct FullNodeTypesAdapter<Types, DB, Provider> {
    /// An instance of the user configured node types.
    pub types: Types,
    /// The database type used by the node.
    pub db: PhantomData<DB>,
    /// The provider type used by the node.
    pub provider: PhantomData<Provider>,
}

impl<Types, DB, Provider> FullNodeTypesAdapter<Types, DB, Provider> {
    /// Create a new adapter from the given node types.
    pub fn new(types: Types) -> Self {
        Self { types, db: Default::default(), provider: Default::default() }
    }
}

impl<Types, DB, Provider> NodeTypes for FullNodeTypesAdapter<Types, DB, Provider>
where
    Types: NodeTypes,
    DB: Send + Sync + 'static,
    Provider: Send + Sync + 'static,
{
    type Primitives = Types::Primitives;
    type Engine = Types::Engine;
    type Evm = Types::Evm;
    type BlockExecutor = Types::BlockExecutor;

    fn evm_config(&self) -> Self::Evm {
        self.types.evm_config()
    }

    fn block_executor(&self) -> Self::BlockExecutor {
        self.types.block_executor()
    }
}

impl<Types, DB, Provider> FullNodeTypes for FullNodeTypesAdapter<Types, DB, Provider>
where
    Types: NodeTypes,
    Provider: FullProvider<DB>,
    DB: Database + Clone + 'static,
{
    type DB = DB;
    type Provider = Provider;
}

/// Encapsulates all types and components of the node.
pub trait FullNodeComponents: FullNodeTypes + 'static {
    /// The transaction pool of the node.
    type Pool: TransactionPool;

    /// Returns the transaction pool of the node.
    fn pool(&self) -> &Self::Pool;

    /// Returns the provider of the node.
    fn provider(&self) -> &Self::Provider;

    /// Returns the handle to the network
    fn network(&self) -> &NetworkHandle;

    /// Returns the handle to the payload builder service.
    fn payload_builder(&self) -> &PayloadBuilderHandle<Self::Engine>;

    /// Returns the task executor.
    fn task_executor(&self) -> &TaskExecutor;
}

/// A type that encapsulates all the components of the node.
#[derive(Debug)]
pub struct FullNodeComponentsAdapter<Node: FullNodeTypes, Pool> {
    /// The EVM configuration of the node.
    pub evm_config: Node::Evm,
    /// The transaction pool of the node.
    pub pool: Pool,
    /// The network handle of the node.
    pub network: NetworkHandle,
    /// The provider of the node.
    pub provider: Node::Provider,
    /// Provides instances of executors to execute blocks, for individual blocks and batches of
    /// blocks (historical sync).
    pub block_executor: Node::BlockExecutor,
    /// The payload builder service handle of the node.
    pub payload_builder: PayloadBuilderHandle<Node::Engine>,
    /// The task executor of the node.
    pub executor: TaskExecutor,
}

impl<Node, Pool> FullNodeTypes for FullNodeComponentsAdapter<Node, Pool>
where
    Node: FullNodeTypes,
    Pool: TransactionPool + 'static,
{
    type DB = Node::DB;
    type Provider = Node::Provider;
}

impl<Node, Pool> NodeTypes for FullNodeComponentsAdapter<Node, Pool>
where
    Node: FullNodeTypes,
    Pool: TransactionPool + 'static,
{
    type Primitives = Node::Primitives;
    type Engine = Node::Engine;
    type Evm = Node::Evm;
    type BlockExecutor = Node::BlockExecutor;

    fn evm_config(&self) -> Self::Evm {
        self.evm_config.clone()
    }

    fn block_executor(&self) -> Self::BlockExecutor {
        self.block_executor.clone()
    }
}

impl<Node, Pool> FullNodeComponents for FullNodeComponentsAdapter<Node, Pool>
where
    Node: FullNodeTypes,
    Pool: TransactionPool + 'static,
{
    type Pool = Pool;

    fn pool(&self) -> &Self::Pool {
        &self.pool
    }

    fn provider(&self) -> &Self::Provider {
        &self.provider
    }

    fn network(&self) -> &NetworkHandle {
        &self.network
    }

    fn payload_builder(&self) -> &PayloadBuilderHandle<Self::Engine> {
        &self.payload_builder
    }

    fn task_executor(&self) -> &TaskExecutor {
        &self.executor
    }
}

impl<Node: FullNodeTypes, Pool> Clone for FullNodeComponentsAdapter<Node, Pool>
where
    Pool: Clone,
{
    fn clone(&self) -> Self {
        Self {
            evm_config: self.evm_config.clone(),
            pool: self.pool.clone(),
            network: self.network.clone(),
            provider: self.provider.clone(),
            block_executor: self.block_executor.clone(),
            payload_builder: self.payload_builder.clone(),
            executor: self.executor.clone(),
        }
    }
}

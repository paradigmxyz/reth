//! Traits for the builder

use crate::{
    components::NodeComponents,
    node::{FullNodeTypes, NodeTypes},
    BuilderContext,
};
use reth_network::NetworkHandle;
use reth_payload_builder::PayloadBuilderHandle;
use reth_tasks::TaskExecutor;
use reth_transaction_pool::TransactionPool;

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
    pub(crate) evm_config: Node::Evm,
    pub(crate) pool: Pool,
    pub(crate) network: NetworkHandle,
    pub(crate) provider: Node::Provider,
    pub(crate) payload_builder: PayloadBuilderHandle<Node::Engine>,
    pub(crate) executor: TaskExecutor,
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

    fn evm_config(&self) -> Self::Evm {
        self.evm_config.clone()
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
            payload_builder: self.payload_builder.clone(),
            executor: self.executor.clone(),
        }
    }
}

/// A type that configures all the customizable components of the node and knows how to build them.
///
/// Implementors of this trait are responsible for building all the components of the node: See
/// [NodeComponents].
///
/// The [ComponentsBuilder](crate::components::builder::ComponentsBuilder) is a generic
/// implementation of this trait that can be used to customize certain components of the node using
/// the builder pattern and defaults, e.g. Ethereum and Optimism.
pub trait NodeComponentsBuilder<Node: FullNodeTypes> {
    /// The transaction pool to use.
    type Pool: TransactionPool + Unpin + 'static;

    /// Builds the components of the node.
    fn build_components(
        self,
        context: &BuilderContext<Node>,
    ) -> eyre::Result<NodeComponents<Node, Self::Pool>>;
}

impl<Node, F, Pool> NodeComponentsBuilder<Node> for F
where
    Node: FullNodeTypes,
    F: FnOnce(&BuilderContext<Node>) -> eyre::Result<NodeComponents<Node, Pool>>,
    Pool: TransactionPool + Unpin + 'static,
{
    type Pool = Pool;

    fn build_components(
        self,
        ctx: &BuilderContext<Node>,
    ) -> eyre::Result<NodeComponents<Node, Pool>> {
        self(ctx)
    }
}

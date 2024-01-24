//! Traits for the builder

use crate::components::{BuilderContext, NodeComponents};
use reth_network::NetworkHandle;
use reth_node_api::node::FullNodeTypes;
use reth_payload_builder::PayloadBuilderHandle;
use reth_tasks::TaskExecutor;
use reth_transaction_pool::TransactionPool;

/// Encapsulates all types and components of the node.
pub trait FullNodeComponents: FullNodeTypes + 'static {
    /// The transaction pool of the node.
    type Pool: TransactionPool;

    /// The events type used to create subscriptions.
    // type Events: CanonStateSubscriptions + Clone + 'static;

    /// Returns the transaction pool of the node.
    fn pool(&self) -> &Self::Pool;

    /// Returns the provider of the node.
    fn provider(&self) -> &Self::Provider;

    /// Returns the handle to the network
    fn network(&self) -> &NetworkHandle;

    /// Returns the handle to the payload builder service.
    fn payload_builder(&self) -> &PayloadBuilderHandle<Self::Engine>;

    /// Returns the task executor.
    fn tasks(&self) -> &TaskExecutor;
}

/// A type that configures all the customizable components of the node and knows how to build them.
///
/// Implementors of this trait are responsible for building all the components of the node: See
/// [NodeComponents].
///
/// The [ComponentsBuilder] is a generic implementation of this trait that can be used to customize
/// certain components of the node using the builder pattern and defaults, e.g. Ethereum and
/// Optimism.
pub trait NodeComponentsBuilder<Node: FullNodeTypes> {
    /// The transaction pool to use.
    type Pool: TransactionPool;

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
    Pool: TransactionPool,
{
    type Pool = Pool;

    fn build_components(
        self,
        ctx: &BuilderContext<Node>,
    ) -> eyre::Result<NodeComponents<Node, Pool>> {
        self(ctx)
    }
}

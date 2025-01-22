//! EVM component for the node builder.
use crate::{BuilderContext, FullNodeTypes};
use reth_evm::{execute::BlockExecutorProvider, ConfigureEvmFor};
use reth_node_api::NodeTypes;
use std::future::Future;

/// A type that knows how to build the executor types.
pub trait ExecutorBuilder<Node: FullNodeTypes>: Send {
    /// The EVM config to use.
    ///
    /// This provides the node with the necessary configuration to configure an EVM.
    type EVM: ConfigureEvmFor<<Node::Types as NodeTypes>::Primitives>;

    /// The type that knows how to execute blocks.
    type Executor: BlockExecutorProvider<Primitives = <Node::Types as NodeTypes>::Primitives>;

    /// Creates the EVM config.
    fn build_evm(
        self,
        ctx: &BuilderContext<Node>,
    ) -> impl Future<Output = eyre::Result<(Self::EVM, Self::Executor)>> + Send;
}

impl<Node, F, Fut, EVM, Executor> ExecutorBuilder<Node> for F
where
    Node: FullNodeTypes,
    EVM: ConfigureEvmFor<<Node::Types as NodeTypes>::Primitives>,
    Executor: BlockExecutorProvider<Primitives = <Node::Types as NodeTypes>::Primitives>,
    F: FnOnce(&BuilderContext<Node>) -> Fut + Send,
    Fut: Future<Output = eyre::Result<(EVM, Executor)>> + Send,
{
    type EVM = EVM;
    type Executor = Executor;

    fn build_evm(
        self,
        ctx: &BuilderContext<Node>,
    ) -> impl Future<Output = eyre::Result<(Self::EVM, Self::Executor)>> {
        self(ctx)
    }
}

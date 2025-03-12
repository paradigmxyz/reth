//! EVM component for the node builder.
use crate::{BuilderContext, ConfigureEvm, FullNodeTypes};
use reth_evm::execute::BlockExecutorProvider;
use reth_node_api::PrimitivesTy;
use std::{fmt::Debug, future::Future};
/// A type that knows how to build the executor types.
pub trait ExecutorBuilder<Node: FullNodeTypes>: Send + Debug {
    /// The EVM config to use.
    ///
    /// This provides the node with the necessary configuration to configure an EVM.
    type EVM: ConfigureEvm<Primitives = PrimitivesTy<Node::Types>> + Debug + 'static;

    /// The type that knows how to execute blocks.
    type Executor: BlockExecutorProvider<Primitives = PrimitivesTy<Node::Types>> + Debug;

    /// Creates the EVM config.
    fn build_evm(
        self,
        ctx: &BuilderContext<Node>,
    ) -> impl Future<Output = eyre::Result<(Self::EVM, Self::Executor)>> + Send;
}

impl<Node, F, Fut, EVM, Executor> ExecutorBuilder<Node> for F
where
    Node: FullNodeTypes,
    EVM: ConfigureEvm<Primitives = PrimitivesTy<Node::Types>> + Debug + 'static,
    Executor: BlockExecutorProvider<Primitives = PrimitivesTy<Node::Types>> + Debug,
    F: FnOnce(&BuilderContext<Node>) -> Fut + Send + Debug,
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

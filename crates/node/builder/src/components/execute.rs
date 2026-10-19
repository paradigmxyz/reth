//! EVM component for the node builder.
use crate::{BuilderContext, ConfigureEvm, FullNodeTypes};
use reth_node_api::PrimitivesTy;
use std::future::Future;

/// A type that knows how to build the executor types.
pub trait ExecutorBuilder<Node: FullNodeTypes>: Send {
    /// The EVM config to use.
    ///
    /// This provides the node with the necessary configuration to configure an EVM.
    type EVM: ConfigureEvm<Primitives = PrimitivesTy<Node::Types>> + 'static;

    /// Creates the EVM config.
    fn build_evm(
        self,
        ctx: &BuilderContext<Node>,
    ) -> impl Future<Output = eyre::Result<Self::EVM>> + Send;
}

impl<Node, F, Fut, EVM> ExecutorBuilder<Node> for F
where
    Node: FullNodeTypes,
    EVM: ConfigureEvm<Primitives = PrimitivesTy<Node::Types>> + 'static,
    F: FnOnce(&BuilderContext<Node>) -> Fut + Send,
    Fut: Future<Output = eyre::Result<EVM>> + Send,
{
    type EVM = EVM;

    fn build_evm(
        self,
        ctx: &BuilderContext<Node>,
    ) -> impl Future<Output = eyre::Result<Self::EVM>> {
        self(ctx)
    }
}

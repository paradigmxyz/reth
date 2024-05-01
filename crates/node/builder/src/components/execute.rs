//! EVM component for the node builder.
use crate::{BuilderContext, FullNodeTypes};
use reth_node_api::ConfigureEvm;
use std::future::Future;

/// A type that knows how to build the executor types.
pub trait ExecutorBuilder<Node: FullNodeTypes>: Send {
    /// The EVM config to build.
    type EVM: ConfigureEvm;
    // TODO(mattsse): integrate `Executor`

    /// Creates the transaction pool.
    fn build_evm(
        self,
        ctx: &BuilderContext<Node>,
    ) -> impl Future<Output = eyre::Result<Self::EVM>> + Send;
}

impl<Node, F, Fut, EVM> ExecutorBuilder<Node> for F
where
    Node: FullNodeTypes,
    EVM: ConfigureEvm,
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

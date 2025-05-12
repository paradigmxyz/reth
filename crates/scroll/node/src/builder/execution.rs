use reth_node_builder::{components::ExecutorBuilder, BuilderContext, FullNodeTypes};
use reth_node_types::NodeTypes;
use reth_scroll_chainspec::ScrollChainSpec;
use reth_scroll_evm::ScrollEvmConfig;
use reth_scroll_primitives::ScrollPrimitives;

/// Executor builder for Scroll.
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct ScrollExecutorBuilder;

impl<Node> ExecutorBuilder<Node> for ScrollExecutorBuilder
where
    Node: FullNodeTypes,
    Node::Types: NodeTypes<ChainSpec = ScrollChainSpec, Primitives = ScrollPrimitives>,
{
    type EVM = ScrollEvmConfig;

    async fn build_evm(self, ctx: &BuilderContext<Node>) -> eyre::Result<Self::EVM> {
        let evm_config = ScrollEvmConfig::scroll(ctx.chain_spec());

        Ok(evm_config)
    }
}

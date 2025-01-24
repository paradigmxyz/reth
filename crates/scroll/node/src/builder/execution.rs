use reth_evm::execute::BasicBlockExecutorProvider;
use reth_node_builder::{components::ExecutorBuilder, BuilderContext, FullNodeTypes};
use reth_node_types::NodeTypesWithEngine;
use reth_scroll_chainspec::ScrollChainSpec;
use reth_scroll_evm::{ScrollEvmConfig, ScrollExecutionStrategyFactory};
use reth_scroll_primitives::ScrollPrimitives;

/// Executor builder for Scroll.
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct ScrollExecutorBuilder;

impl<Node> ExecutorBuilder<Node> for ScrollExecutorBuilder
where
    Node: FullNodeTypes,
    Node::Types: NodeTypesWithEngine<ChainSpec = ScrollChainSpec, Primitives = ScrollPrimitives>,
{
    type EVM = ScrollEvmConfig;
    type Executor = BasicBlockExecutorProvider<ScrollExecutionStrategyFactory>;

    async fn build_evm(
        self,
        ctx: &BuilderContext<Node>,
    ) -> eyre::Result<(Self::EVM, Self::Executor)> {
        let evm_config = ScrollEvmConfig::new(ctx.chain_spec());
        let strategy_factory = ScrollExecutionStrategyFactory::scroll(ctx.chain_spec());
        let executor = BasicBlockExecutorProvider::new(strategy_factory);

        Ok((evm_config, executor))
    }
}

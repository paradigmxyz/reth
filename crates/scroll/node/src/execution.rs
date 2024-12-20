use reth_evm::execute::BasicBlockExecutorProvider;
use reth_node_builder::{components::ExecutorBuilder, BuilderContext, FullNodeTypes};
use reth_node_types::NodeTypesWithEngine;
use reth_primitives::EthPrimitives;
use reth_scroll_chainspec::ScrollChainSpec;
use reth_scroll_evm::{ScrollEvmConfig, ScrollExecutionStrategyFactory};

/// Executor builder for Scroll.
#[derive(Debug, Default)]
pub struct ScrollExecutorBuilder;

impl<Node> ExecutorBuilder<Node> for ScrollExecutorBuilder
where
    Node: FullNodeTypes,
    Node::Types: NodeTypesWithEngine<ChainSpec = ScrollChainSpec, Primitives = EthPrimitives>,
{
    type EVM = ScrollEvmConfig;
    type Executor = BasicBlockExecutorProvider<ScrollExecutionStrategyFactory>;

    async fn build_evm(
        self,
        ctx: &BuilderContext<Node>,
    ) -> eyre::Result<(Self::EVM, Self::Executor)> {
        let chain_spec = ctx.chain_spec();
        let strategy_factory = ScrollExecutionStrategyFactory::new(chain_spec);
        let evm_config = strategy_factory.evm_config();

        let executor = BasicBlockExecutorProvider::new(strategy_factory);

        Ok((evm_config, executor))
    }
}

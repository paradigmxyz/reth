use crate::evm::WormholeEvmConfig;
use reth_node_api::FullNodeTypes;
use reth_node_builder::{components::ExecutorBuilder, BuilderContext, NodeTypes};
use reth_op::chainspec::OpChainSpec;

#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct WormholeExecutorBuilder;

impl<Node> ExecutorBuilder<Node> for WormholeExecutorBuilder
where
    Node: FullNodeTypes<
        Types: NodeTypes<ChainSpec = OpChainSpec, Primitives = reth_op::OpPrimitives>,
    >,
{
    type EVM = WormholeEvmConfig;

    async fn build_evm(self, ctx: &BuilderContext<Node>) -> eyre::Result<Self::EVM> {
        Ok(WormholeEvmConfig::new(ctx.chain_spec()))
    }
}

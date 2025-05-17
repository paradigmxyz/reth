use crate::{
    chainspec::CustomChainSpec,
    evm::{CustomBlockAssembler, CustomEvmConfig},
    primitives::CustomNodePrimitives,
};
use reth_ethereum::node::api::{FullNodeTypes, NodeTypes};
use reth_node_builder::{components::ExecutorBuilder, BuilderContext};
use reth_optimism_evm::{OpBlockAssembler, OpEvmConfig, OpRethReceiptBuilder};
use std::sync::Arc;

type ChainSpec = CustomChainSpec;
type Primitives = CustomNodePrimitives;

/// A custom executor builder
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct CustomExecutorBuilder;

impl<Types, Node> ExecutorBuilder<Node> for CustomExecutorBuilder
where
    Types: NodeTypes<ChainSpec = ChainSpec, Primitives = Primitives>,
    Node: FullNodeTypes<Types = Types>,
{
    type EVM = CustomEvmConfig;

    async fn build_evm(self, ctx: &BuilderContext<Node>) -> eyre::Result<Self::EVM> {
        let evm_config = CustomEvmConfig::new(
            OpEvmConfig::new(
                Arc::new(ctx.chain_spec().inner().clone()),
                OpRethReceiptBuilder::default(),
            ),
            CustomBlockAssembler::new(OpBlockAssembler::new(ctx.chain_spec())),
        );

        Ok(evm_config)
    }
}

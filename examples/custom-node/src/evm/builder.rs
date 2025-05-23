use crate::{
    chainspec::CustomChainSpec,
    evm::{alloy::CustomEvmFactory, CustomBlockAssembler, CustomEvmConfig},
    primitives::CustomNodePrimitives,
};
use alloy_op_evm::OpEvmFactory;
use reth_ethereum::node::api::FullNodeTypes;
use reth_node_builder::{components::ExecutorBuilder, BuilderContext, NodeTypes};
use reth_op::node::{OpBlockAssembler, OpEvmConfig, OpRethReceiptBuilder};
use std::{future, future::Future, sync::Arc};

pub struct CustomExecutorBuilder;

impl<Node: FullNodeTypes> ExecutorBuilder<Node> for CustomExecutorBuilder
where
    Node::Types: NodeTypes<ChainSpec = CustomChainSpec, Primitives = CustomNodePrimitives>,
{
    type EVM = CustomEvmConfig;

    fn build_evm(
        self,
        ctx: &BuilderContext<Node>,
    ) -> impl Future<Output = eyre::Result<Self::EVM>> + Send {
        future::ready(Ok(CustomEvmConfig::new(
            OpEvmConfig::new(
                Arc::new(ctx.chain_spec().inner().clone()),
                OpRethReceiptBuilder::default(),
            ),
            CustomBlockAssembler::new(OpBlockAssembler::new(ctx.chain_spec())),
            CustomEvmFactory::new(OpEvmFactory::default()),
        )))
    }
}

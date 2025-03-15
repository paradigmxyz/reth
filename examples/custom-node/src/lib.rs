//! This example shows how implement a custom node.
//!
//! A node consists of:
//! - primtives: block,header,transactions
//! - components: network,pool,evm
//! - engine: advances the node

#![cfg_attr(not(test), warn(unused_crate_dependencies))]
// The `optimism` feature must be enabled to use this crate.
//#![cfg(feature = "optimism")]

use chainspec::CustomChainSpec;
use engine::CustomEngineTypes;
use evm::CustomEvmConfig;
use op_alloy_consensus::OpPooledTransaction;
use primitives::{Block, BlockBody, CustomHeader, CustomNodePrimitives};
use reth_eth_wire_types::NetworkPrimitives;
use reth_evm::execute::BasicBlockExecutorProvider;
use reth_node_api::{FullNodeComponents, FullNodeTypes, NodeTypes, NodeTypesWithEngine};
use reth_node_builder::{
    components::{ComponentsBuilder, ExecutorBuilder, PayloadServiceBuilder},
    Node,
};
use reth_optimism_node::{
    node::{OpAddOns, OpConsensusBuilder, OpNetworkBuilder, OpPoolBuilder, OpStorage},
    OpNode,
};
use reth_optimism_primitives::{OpReceipt, OpTransactionSigned};
use reth_payload_builder::PayloadBuilderHandle;
use txpool::CustomTxPool;

pub mod chainspec;
pub mod engine;
pub mod evm;
pub mod primitives;
pub mod txpool;

#[derive(Debug, Clone)]
pub struct CustomNode(OpNode);

impl NodeTypes for CustomNode {
    type Primitives = CustomNodePrimitives;
    type ChainSpec = CustomChainSpec;
    type StateCommitment = <OpNode as NodeTypes>::StateCommitment;
    type Storage = <OpNode as NodeTypes>::Storage;
}

impl NodeTypesWithEngine for CustomNode {
    type Engine = CustomEngineTypes;
}

type CustomAddOns<N> = OpAddOns<N>;

impl<N> Node<N> for CustomNode
where
    N: FullNodeTypes<
            Types: NodeTypesWithEngine<
                Engine = CustomEngineTypes,
                ChainSpec = CustomChainSpec,
                Primitives = CustomNodePrimitives,
                Storage = OpStorage,
            >,
        > + FullNodeComponents,
{
    type ComponentsBuilder = ComponentsBuilder<
        N,
        OpPoolBuilder,
        CustomPayloadServiceBuilder,
        OpNetworkBuilder,
        CustomExecutorBuilder,
        OpConsensusBuilder,
    >;

    type AddOns = CustomAddOns<N>;

    fn components_builder(&self) -> Self::ComponentsBuilder {
        todo!()
    }

    fn add_ons(&self) -> Self::AddOns {
        CustomAddOns::default()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
struct CustomNetworkPrimitives;

/// Custom payload builder.
struct CustomPayloadServiceBuilder(reth_optimism_node::node::OpPayloadBuilder);

impl<Node> PayloadServiceBuilder<Node, CustomTxPool<Node::Provider>> for CustomPayloadServiceBuilder
where
    Node: FullNodeTypes<Types = CustomNode>,
{
    async fn spawn_payload_builder_service(
        self,
        ctx: &reth_node_builder::BuilderContext<Node>,
        pool: CustomTxPool<Node::Provider>,
    ) -> eyre::Result<
        PayloadBuilderHandle<<<Node as FullNodeTypes>::Types as NodeTypesWithEngine>::Engine>,
    > {
    }
}

#[derive(Debug, Clone, Default)]
struct CustomExecutorBuilder;

impl<Node> ExecutorBuilder<Node> for CustomExecutorBuilder
where
    Node: FullNodeTypes<Types = CustomNode>,
{
    type EVM = CustomEvmConfig;
    type Executor = BasicBlockExecutorProvider<Self::EVM>;

    async fn build_evm(
        self,
        ctx: &reth_node_builder::BuilderContext<Node>,
    ) -> eyre::Result<(Self::EVM, Self::Executor)> {
        let evm_config = CustomEvmConfig::new(ctx.chain_spec().clone());
        let executor = BasicBlockExecutorProvider::new(evm_config.clone());

        Ok((evm_config, executor))
    }
}

impl NetworkPrimitives for CustomNetworkPrimitives {
    type BlockHeader = CustomHeader;
    type BlockBody = BlockBody;
    type Block = Block;
    type BroadcastedTransaction = OpTransactionSigned;
    type PooledTransaction = OpPooledTransaction;
    type Receipt = OpReceipt;
}

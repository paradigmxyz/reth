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
use engine::{CustomEngineTypes, CustomPayloadBuilder};
use evm::CustomEvmConfig;
use op_alloy_consensus::OpPooledTransaction;
use primitives::{Block, BlockBody, CustomHeader, CustomNodePrimitives};
use reth_eth_wire_types::NetworkPrimitives;
use reth_evm::execute::BasicBlockExecutorProvider;
use reth_node_api::{FullNodeTypes, NodeTypes, NodeTypesWithEngine};
use reth_node_builder::{
    components::{ComponentsBuilder, ExecutorBuilder, PayloadServiceBuilder},
    rpc::RpcAddOns,
    Node, NodeAdapter, NodeComponentsBuilder,
};
use reth_optimism_node::{
    node::{
        OpAddOns, OpConsensusBuilder, OpEngineValidatorBuilder, OpNetworkBuilder, OpPoolBuilder,
        OpStorage,
    },
    OpEngineApiBuilder, OpNode,
};
use reth_optimism_primitives::{OpReceipt, OpTransactionSigned};
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
    >,
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
    type PayloadBuilder = CustomPayloadBuilder<Node::Provider>;

    async fn build_payload_builder(
        &self,
        ctx: &reth_node_builder::BuilderContext<Node>,
        pool: CustomTxPool<Node::Provider>,
    ) -> eyre::Result<Self::PayloadBuilder> {
        Ok(CustomPayloadBuilder::new(pool, ctx.provider().clone(), ctx.chain_spec().clone()))
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

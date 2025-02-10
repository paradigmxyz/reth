//! This example shows how implement a custom node.
//!
//! A node consists of:
//! - primtives: block,header,transactions
//! - components: network,pool,evm
//! - engine: advances the node

#![cfg_attr(not(test), warn(unused_crate_dependencies))]
// The `optimism` feature must be enabled to use this crate.
#![cfg(feature = "optimism")]

use chainspec::CustomChainSpec;
use engine::{CustomEngineTypes, CustomPayloadBuilder};
use evm::CustomEvmConfig;
use op_alloy_consensus::OpPooledTransaction;
use reth_optimism_node::{BasicOpReceiptBuilder, OpNetworkPrimitives};
use primitives::{Block, BlockBody, CustomHeader, CustomNodePrimitives};
use reth_chainspec::ChainSpecProvider;
use reth_evm::execute::BasicBlockExecutorProvider;
use reth_eth_wire_types::NetworkPrimitives;
use reth_node_api::{FullNodeTypes, NodeTypes, NodeTypesWithEngine, PrimitivesTy, TxTy};
use reth_node_builder::{
    components::{ComponentsBuilder, ExecutorBuilder, NetworkBuilder, PayloadServiceBuilder},
    Node, NodeAdapter, NodeComponentsBuilder,
};
use reth_optimism_node::{
    engine::OpPayloadTypes,
    node::{
        OpAddOns, OpConsensusBuilder, OpExecutorBuilder, OpNetworkBuilder, OpPayloadBuilder,
        OpPoolBuilder,
    },
    OpEngineTypes, OpEvmConfig, OpExecutionStrategyFactory, OpNode,
};
use reth_optimism_primitives::{OpReceipt, OpTransactionSigned};
use reth_storage_api::EthStorage;
use reth_transaction_pool::{PoolTransaction, TransactionPool};
use reth_trie_db::MerklePatriciaTrie;
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

impl<N: FullNodeTypes<Types = Self>> Node<N> for CustomNode {
    type ComponentsBuilder = ComponentsBuilder<
        N,
        OpPoolBuilder,
        CustomPayloadServiceBuilder,
        OpNetworkBuilder<CustomNetworkPrimitives>,
        CustomExecutorBuilder,
        OpConsensusBuilder,
    >;

    type AddOns =
        OpAddOns<NodeAdapter<N, <Self::ComponentsBuilder as
NodeComponentsBuilder<N>>::Components>>;

    fn components_builder(&self) -> Self::ComponentsBuilder {
        todo!()
    }

    fn add_ons(&self) -> Self::AddOns {
        todo!()
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
    type Executor = BasicBlockExecutorProvider<
        OpExecutionStrategyFactory<PrimitivesTy<Node::Types>, CustomChainSpec, CustomEvmConfig>,
    >;

    async fn build_evm(
        self,
        ctx: &reth_node_builder::BuilderContext<Node>,
    ) -> eyre::Result<(Self::EVM, Self::Executor)> {
        let chain_spec = ctx.provider().chain_spec();
        let evm_config = CustomEvmConfig::new(ctx.chain_spec().clone());
        let strategy_factory = OpExecutionStrategyFactory::new(
            chain_spec,
            evm_config.clone(),
            BasicOpReceiptBuilder::default(),
        );
        let executor = BasicBlockExecutorProvider::new(strategy_factory);

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

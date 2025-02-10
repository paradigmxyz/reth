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
use primitives::CustomNodePrimitives;
use reth_node_api::{FullNodeTypes, NodeTypes, NodeTypesWithEngine, PrimitivesTy, TxTy};
use reth_node_builder::{
    components::{ComponentsBuilder, PayloadServiceBuilder},
    Node, NodeAdapter, NodeComponentsBuilder,
};
use reth_optimism_node::{
    engine::OpPayloadTypes,
    node::{
        OpAddOns, OpConsensusBuilder, OpExecutorBuilder, OpNetworkBuilder, OpPayloadBuilder,
        OpPoolBuilder,
    },
    OpEngineTypes, OpEvmConfig, OpNode,
};
use reth_storage_api::EthStorage;
use reth_transaction_pool::{PoolTransaction, TransactionPool};
use reth_trie_db::MerklePatriciaTrie;
use txpool::CustomTxPool;

pub mod chainspec;
pub mod engine;
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

// impl<N: FullNodeTypes<Types = Self>> Node<N> for CustomNode {
//     type ComponentsBuilder = ComponentsBuilder<
//         N,
//         OpPoolBuilder,
//         OpPayloadBuilder,
//         OpNetworkBuilder,
//         OpExecutorBuilder,
//         OpConsensusBuilder,
//     >;

//     type AddOns =
//         OpAddOns<NodeAdapter<N, <Self::ComponentsBuilder as
// NodeComponentsBuilder<N>>::Components>>;

//     fn components_builder(&self) -> Self::ComponentsBuilder {
//         self.0.components()
//     }

//     fn add_ons(&self) -> Self::AddOns {
//         self.0.add_ons()
//     }
// }

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

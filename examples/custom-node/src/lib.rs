//! This example shows how implement a custom node.
//!
//! A node consists of:
//! - primitives: block,header,transactions
//! - components: network,pool,evm
//! - engine: advances the node

#![cfg_attr(not(test), warn(unused_crate_dependencies))]

use chainspec::CustomChainSpec;
use engine::CustomPayloadTypes;
use primitives::CustomNodePrimitives;
use reth_ethereum::node::api::{FullNodeTypes, NodeTypes};
use reth_node_builder::{components::ComponentsBuilder, Node, NodeComponentsBuilder};
use reth_op::node::{
    node::{OpConsensusBuilder, OpPoolBuilder, OpStorage},
    OpNode,
};

pub mod chainspec;
pub mod engine;
pub mod engine_api;
pub mod evm;
pub mod network;
pub mod primitives;

#[derive(Debug, Clone)]
pub struct CustomNode {}

impl NodeTypes for CustomNode {
    type Primitives = CustomNodePrimitives;
    type ChainSpec = CustomChainSpec;
    type StateCommitment = <OpNode as NodeTypes>::StateCommitment;
    type Storage = <OpNode as NodeTypes>::Storage;
    type Payload = CustomPayloadTypes;
}

impl<N> Node<N> for CustomNode
where
    N: FullNodeTypes<
        Types: NodeTypes<
            Payload = CustomPayloadTypes,
            ChainSpec = CustomChainSpec,
            Primitives = CustomNodePrimitives,
            Storage = OpStorage,
        >,
    >,
    ComponentsBuilder<N, OpPoolBuilder, (), (), (), OpConsensusBuilder>: NodeComponentsBuilder<N>,
{
    type ComponentsBuilder = ComponentsBuilder<N, OpPoolBuilder, (), (), (), OpConsensusBuilder>;

    type AddOns = ();

    fn components_builder(&self) -> Self::ComponentsBuilder {
        ComponentsBuilder::default()
            .node_types::<N>()
            .pool(OpPoolBuilder::default())
            .consensus(OpConsensusBuilder::default())
    }

    fn add_ons(&self) -> Self::AddOns {}
}

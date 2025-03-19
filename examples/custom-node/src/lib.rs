//! This example shows how implement a custom node.
//!
//! A node consists of:
//! - primtives: block,header,transactions
//! - components: network,pool,evm
//! - engine: advances the node

#![cfg_attr(not(test), warn(unused_crate_dependencies))]

use chainspec::CustomChainSpec;
use engine::CustomEngineTypes;
use primitives::CustomNodePrimitives;
use reth_node_api::{FullNodeTypes, NodeTypes, NodeTypesWithEngine};
use reth_node_builder::{components::ComponentsBuilder, Node, NodeComponentsBuilder};
use reth_optimism_node::{
    node::{OpConsensusBuilder, OpPoolBuilder, OpStorage},
    OpNode,
};

pub mod chainspec;
pub mod engine;
pub mod primitives;

#[derive(Debug, Clone)]
pub struct CustomNode {}

impl NodeTypes for CustomNode {
    type Primitives = CustomNodePrimitives;
    type ChainSpec = CustomChainSpec;
    type StateCommitment = <OpNode as NodeTypes>::StateCommitment;
    type Storage = <OpNode as NodeTypes>::Storage;
}

impl NodeTypesWithEngine for CustomNode {
    type Engine = CustomEngineTypes;
}

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

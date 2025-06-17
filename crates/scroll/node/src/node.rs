//! Node specific implementations for Scroll.

use crate::{
    ScrollAddOns, ScrollConsensusBuilder, ScrollExecutorBuilder, ScrollNetworkBuilder,
    ScrollPayloadBuilderBuilder, ScrollPoolBuilder, ScrollStorage,
};
use reth_node_builder::{
    components::{BasicPayloadServiceBuilder, ComponentsBuilder},
    node::{FullNodeTypes, NodeTypes},
    Node, NodeAdapter, NodeComponentsBuilder,
};
use reth_scroll_chainspec::ScrollChainSpec;
use reth_scroll_engine_primitives::ScrollEngineTypes;
use reth_scroll_primitives::ScrollPrimitives;
use reth_trie_db::MerklePatriciaTrie;

/// The Scroll node implementation.
#[derive(Clone, Debug, Default)]
pub struct ScrollNode;

impl ScrollNode {
    /// Returns a [`ComponentsBuilder`] configured for a regular Ethereum node.
    pub fn components<Node>() -> ComponentsBuilder<
        Node,
        ScrollPoolBuilder,
        BasicPayloadServiceBuilder<ScrollPayloadBuilderBuilder>,
        ScrollNetworkBuilder,
        ScrollExecutorBuilder,
        ScrollConsensusBuilder,
    >
    where
        Node: FullNodeTypes<
            Types: NodeTypes<
                ChainSpec = ScrollChainSpec,
                Primitives = ScrollPrimitives,
                Payload = ScrollEngineTypes,
            >,
        >,
    {
        ComponentsBuilder::default()
            .node_types::<Node>()
            .pool(ScrollPoolBuilder::default())
            .executor(ScrollExecutorBuilder::default())
            .payload(BasicPayloadServiceBuilder::new(ScrollPayloadBuilderBuilder::default()))
            .network(ScrollNetworkBuilder)
            .executor(ScrollExecutorBuilder)
            .consensus(ScrollConsensusBuilder)
    }
}

impl<N> Node<N> for ScrollNode
where
    N: FullNodeTypes<Types = Self>,
{
    type ComponentsBuilder = ComponentsBuilder<
        N,
        ScrollPoolBuilder,
        BasicPayloadServiceBuilder<ScrollPayloadBuilderBuilder>,
        ScrollNetworkBuilder,
        ScrollExecutorBuilder,
        ScrollConsensusBuilder,
    >;

    type AddOns = ScrollAddOns<
        NodeAdapter<N, <Self::ComponentsBuilder as NodeComponentsBuilder<N>>::Components>,
    >;

    fn components_builder(&self) -> Self::ComponentsBuilder {
        Self::components()
    }

    fn add_ons(&self) -> Self::AddOns {
        ScrollAddOns::default()
    }
}

impl NodeTypes for ScrollNode {
    type Primitives = ScrollPrimitives;
    type ChainSpec = ScrollChainSpec;
    type StateCommitment = MerklePatriciaTrie;
    type Storage = ScrollStorage;
    type Payload = ScrollEngineTypes;
}

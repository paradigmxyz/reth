//! Node specific implementations for Scroll.
#![cfg(all(feature = "scroll", not(feature = "optimism")))]

use crate::{
    ScrollAddOns, ScrollConsensusBuilder, ScrollExecutorBuilder, ScrollNetworkBuilder,
    ScrollPayloadBuilder, ScrollPoolBuilder, ScrollStorage,
};
use reth_ethereum_engine_primitives::{
    EthBuiltPayload, EthEngineTypes, EthPayloadAttributes, EthPayloadBuilderAttributes,
};
use reth_node_builder::{
    components::ComponentsBuilder,
    node::{FullNodeTypes, NodeTypes, NodeTypesWithEngine},
    Node, NodeAdapter, NodeComponentsBuilder, PayloadTypes,
};
use reth_primitives::EthPrimitives;
use reth_scroll_chainspec::ScrollChainSpec;
use reth_trie_db::MerklePatriciaTrie;

/// The Scroll node implementation.
#[derive(Clone, Debug)]
pub struct ScrollNodeMpt;

impl ScrollNodeMpt {
    /// Returns a [`ComponentsBuilder`] configured for a regular Ethereum node.
    pub fn components<Node>() -> ComponentsBuilder<
        Node,
        ScrollPoolBuilder,
        ScrollPayloadBuilder,
        ScrollNetworkBuilder,
        ScrollExecutorBuilder,
        ScrollConsensusBuilder,
    >
    where
        Node: FullNodeTypes<
            Types: NodeTypes<ChainSpec = ScrollChainSpec, Primitives = EthPrimitives>,
        >,
        <Node::Types as NodeTypesWithEngine>::Engine: PayloadTypes<
            BuiltPayload = EthBuiltPayload,
            PayloadAttributes = EthPayloadAttributes,
            PayloadBuilderAttributes = EthPayloadBuilderAttributes,
        >,
    {
        ComponentsBuilder::default()
            .node_types::<Node>()
            .pool(ScrollPoolBuilder)
            .payload(ScrollPayloadBuilder)
            .network(ScrollNetworkBuilder)
            .executor(ScrollExecutorBuilder)
            .consensus(ScrollConsensusBuilder)
    }
}

impl<N> Node<N> for ScrollNodeMpt
where
    N: FullNodeTypes<Types = Self>,
{
    type ComponentsBuilder = ComponentsBuilder<
        N,
        ScrollPoolBuilder,
        ScrollPayloadBuilder,
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

impl NodeTypesWithEngine for ScrollNodeMpt {
    type Engine = EthEngineTypes;
}

impl NodeTypes for ScrollNodeMpt {
    // TODO(scroll): update to scroll primitives when we introduce the revm SDK pattern.
    type Primitives = EthPrimitives;
    type ChainSpec = ScrollChainSpec;
    type StateCommitment = MerklePatriciaTrie;
    type Storage = ScrollStorage;
}

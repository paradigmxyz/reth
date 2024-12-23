//! Node specific implementations for Scroll.
#![cfg(all(feature = "scroll", not(feature = "optimism")))]

use reth_ethereum_engine_primitives::{
    EthBuiltPayload, EthPayloadAttributes, EthPayloadBuilderAttributes,
};
use reth_node_builder::{
    components::ComponentsBuilder,
    node::{FullNodeTypes, NodeTypes, NodeTypesWithEngine},
    Node, NodeAdapter, NodeComponentsBuilder, PayloadTypes,
};
use reth_primitives::EthPrimitives;
use reth_scroll_chainspec::ScrollChainSpec;

mod addons;
pub use addons::ScrollAddOns;

mod consensus;
pub use consensus::ScrollConsensusBuilder;

mod engine;
pub use engine::ScrollEngineValidatorBuilder;

mod execution;
pub use execution::ScrollExecutorBuilder;

mod network;
pub use network::ScrollNetworkBuilder;

mod payload;
pub use payload::ScrollPayloadBuilder;

mod pool;
pub use pool::ScrollPoolBuilder;

mod storage;
pub use storage::ScrollStorage;

mod types;

/// The Scroll node implementation.
#[derive(Clone, Debug)]
pub struct ScrollNode;

impl ScrollNode {
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

impl<N> Node<N> for ScrollNode
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

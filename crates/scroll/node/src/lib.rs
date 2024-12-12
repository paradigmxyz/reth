//! Node specific implementations for Scroll.
#![cfg(all(feature = "scroll", not(feature = "optimism")))]

use reth_ethereum_engine_primitives::EthEngineTypes;
use reth_node_builder::{
    components::ComponentsBuilder, FullNodeTypes, Node, NodeAdapter, NodeComponentsBuilder,
};
use reth_node_types::{NodeTypesWithDB, NodeTypesWithEngine};
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

impl<N> Node<N> for ScrollNode
where
    N: FullNodeTypes,
    N::Types: NodeTypesWithDB
        + NodeTypesWithEngine<
            ChainSpec = ScrollChainSpec,
            Primitives = EthPrimitives,
            Engine = EthEngineTypes,
            Storage = ScrollStorage,
        >,
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
        ComponentsBuilder::default()
            .node_types::<N>()
            .pool(ScrollPoolBuilder)
            .payload(ScrollPayloadBuilder)
            .network(ScrollNetworkBuilder)
            .executor(ScrollExecutorBuilder)
            .consensus(ScrollConsensusBuilder)
    }

    fn add_ons(&self) -> Self::AddOns {
        ScrollAddOns::default()
    }
}

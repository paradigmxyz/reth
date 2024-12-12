use crate::{storage::ScrollStorage, ScrollNode};
use reth_ethereum_engine_primitives::EthEngineTypes;
use reth_node_types::{NodeTypes, NodeTypesWithEngine};
use reth_primitives::EthPrimitives;
use reth_scroll_chainspec::ScrollChainSpec;
use reth_scroll_state_commitment::BinaryMerklePatriciaTrie;

impl NodeTypesWithEngine for ScrollNode {
    type Engine = EthEngineTypes;
}

impl NodeTypes for ScrollNode {
    // TODO(scroll): update to scroll primitives when we introduce the revm SDK pattern.
    type Primitives = EthPrimitives;
    type ChainSpec = ScrollChainSpec;
    type StateCommitment = BinaryMerklePatriciaTrie;
    type Storage = ScrollStorage;
}

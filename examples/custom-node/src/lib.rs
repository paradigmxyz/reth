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
use engine::CustomEngineTypes;
use primitives::CustomNodePrimitives;
use reth_node_api::{NodeTypes, NodeTypesWithEngine};
use reth_optimism_node::{engine::OpPayloadTypes, OpEngineTypes, OpNode};
use reth_storage_api::EthStorage;
use reth_trie_db::MerklePatriciaTrie;

pub mod chainspec;
pub mod engine;
pub mod primitives;

pub struct CustomNode;

impl NodeTypes for CustomNode {
    type Primitives = CustomNodePrimitives;
    type ChainSpec = CustomChainSpec;
    type StateCommitment = <OpNode as NodeTypes>::StateCommitment;
    type Storage = <OpNode as NodeTypes>::Storage;
}

impl NodeTypesWithEngine for CustomNode {
    type Engine = CustomEngineTypes;
}


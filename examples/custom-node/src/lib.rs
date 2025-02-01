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
use primitives::CustomNodePrimitives;
use reth_node_api::NodeTypes;
use reth_storage_api::EthStorage;
use reth_trie_db::MerklePatriciaTrie;

pub mod chainspec;
pub mod primitives;

pub struct CustomNode;

impl NodeTypes for CustomNode {
    type Primitives = CustomNodePrimitives;
    type ChainSpec = CustomChainSpec;
    type StateCommitment = MerklePatriciaTrie;
    type Storage = EthStorage;
}

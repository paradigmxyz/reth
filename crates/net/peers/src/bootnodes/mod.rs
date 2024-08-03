//! Bootnodes for the network

use crate::NodeRecord;

mod ethereum;
pub use ethereum::*;

mod optimism;
pub use optimism::*;

/// Returns parsed mainnet nodes
pub fn mainnet_nodes() -> Vec<NodeRecord> {
    parse_nodes(&MAINNET_BOOTNODES[..])
}

/// Returns parsed sepolia nodes
pub fn sepolia_nodes() -> Vec<NodeRecord> {
    parse_nodes(&SEPOLIA_BOOTNODES[..])
}

/// Returns parsed holesky nodes
pub fn holesky_nodes() -> Vec<NodeRecord> {
    parse_nodes(&HOLESKY_BOOTNODES[..])
}

/// Returns parsed op-stack mainnet nodes
pub fn op_nodes() -> Vec<NodeRecord> {
    parse_nodes(OP_BOOTNODES)
}

/// Returns parsed op-stack testnet nodes
pub fn op_testnet_nodes() -> Vec<NodeRecord> {
    parse_nodes(OP_TESTNET_BOOTNODES)
}

/// Returns parsed op-stack base mainnet nodes
pub fn base_nodes() -> Vec<NodeRecord> {
    parse_nodes(OP_BOOTNODES)
}

/// Returns parsed op-stack base testnet nodes
pub fn base_testnet_nodes() -> Vec<NodeRecord> {
    parse_nodes(OP_TESTNET_BOOTNODES)
}

/// Parses all the nodes
pub fn parse_nodes(nodes: impl IntoIterator<Item = impl AsRef<str>>) -> Vec<NodeRecord> {
    nodes.into_iter().map(|s| s.as_ref().parse().unwrap()).collect()
}

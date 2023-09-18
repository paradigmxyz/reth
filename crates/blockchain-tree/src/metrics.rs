use reth_metrics::{
    metrics::{Counter, Gauge},
    Metrics,
};

/// Metrics for the entire blockchain tree
#[derive(Metrics)]
#[metrics(scope = "blockchain_tree")]
pub struct TreeMetrics {
    /// Total number of sidechains (not including the canonical chain)
    pub sidechains: Gauge,
    /// The highest block number in the canonical chain
    pub canonical_chain_height: Gauge,
    /// The number of reorgs
    pub reorgs: Counter,
    /// The latest reorg depth
    pub latest_reorg_depth: Gauge,
    /// Longest sidechain height
    pub longest_sidechain_height: Gauge,
}

/// Metrics for the blockchain tree block buffer
#[derive(Metrics)]
#[metrics(scope = "blockchain_tree.block_buffer")]
pub struct BlockBufferMetrics {
    /// Total blocks in the block buffer
    pub blocks: Gauge,
}

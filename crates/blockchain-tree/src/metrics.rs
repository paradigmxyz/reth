use reth_metrics::{
    metrics::{self, Gauge, Histogram},
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
}

/// Metrics for the blockchain tree block buffer
#[derive(Metrics)]
#[metrics(scope = "blockchain_tree.block_buffer")]
pub struct BlockBufferMetrics {
    /// Total blocks in the block buffer
    pub blocks: Gauge,
    /// How long it takes to re-insert one buffered block in milliseconds.
    pub re_insert_one_buffered_block_ms: Histogram,
}

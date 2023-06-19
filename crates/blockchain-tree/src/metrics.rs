use reth_metrics::{metrics, Metrics};

/// Metrics for the entire blockchain tree
#[derive(Metrics)]
#[metrics(scope = "blockchain_tree")]
pub struct TreeMetrics {}

/// Metrics for the blockchain tree block buffer
#[derive(Metrics)]
#[metrics(scope = "blockchain_tree.block_buffer")]
pub struct BlockBufferMetrics {}

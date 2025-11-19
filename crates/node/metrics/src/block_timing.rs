//! Block timing metrics for tracking block production and execution times

use alloy_primitives::B256;
use indexmap::IndexMap;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

/// Timing metrics for block building phase
#[derive(Debug, Clone, Default)]
pub struct BuildTiming {
    /// Time spent applying pre-execution changes
    pub apply_pre_execution_changes: Duration,
    /// Time spent executing sequencer transactions
    pub execute_sequencer_transactions: Duration,
    /// Time spent executing mempool transactions
    pub execute_mempool_transactions: Duration,
    /// Time spent finishing block build (state root calculation, etc.)
    pub finish: Duration,
    /// Total build time
    pub total: Duration,
}

/// Timing metrics for block insertion phase
#[derive(Debug, Clone, Default)]
pub struct InsertTiming {
    /// Time spent validating and executing the block
    pub validate_and_execute: Duration,
    /// Time spent inserting to tree state
    pub insert_to_tree: Duration,
    /// Total insert time
    pub total: Duration,
}

/// Timing metrics for transaction execution
/// 
/// Note: Individual transaction execution times (sequencer_txs, mempool_txs) are stored
/// in `BuildTiming` to avoid duplication. This struct only stores the total time.
#[derive(Debug, Clone, Default)]
pub struct DeliverTxsTiming {
    /// Total transaction execution time
    /// Note: Individual times are stored in BuildTiming (execute_sequencer_transactions, execute_mempool_transactions)
    pub total: Duration,
}

/// Complete timing metrics for a block
#[derive(Debug, Clone, Default)]
pub struct BlockTimingMetrics {
    /// Block building phase timing
    pub build: BuildTiming,
    /// Block insertion phase timing
    pub insert: InsertTiming,
    /// Transaction execution timing
    pub deliver_txs: DeliverTxsTiming,
}

impl BlockTimingMetrics {
    /// Format timing metrics for logging
    pub fn format_for_log(&self) -> String {
        let format_duration = |d: Duration| {
            let ms = d.as_millis();
            let us = d.as_micros();
            if ms > 0 {
                format!("{}ms", ms)
            } else if us > 0 {
                format!("{}µs", us)
            } else {
                format!("{}ns", d.as_nanos())
            }
        };

        // Check if block was built locally (has build timing) or received from network
        let is_locally_built = self.build.total.as_nanos() > 0;
        
        if is_locally_built {
            // Block was built locally, show full timing including Build and DeliverTxs
            // Note: DeliverTxs only shows total to avoid duplication with Build's seqTxs/mempoolTxs
            let deliver_txs_total_time = self.build.execute_sequencer_transactions + self.build.execute_mempool_transactions;

            format!(
                "Produce[Build[applyPreExec<{}>, seqTxs<{}>, mempoolTxs<{}>, finish<{}>, total<{}>], Insert[validateExec<{}>, insertTree<{}>, total<{}>]], DeliverTxs[total<{}>]",
                format_duration(self.build.apply_pre_execution_changes),
                format_duration(self.build.execute_sequencer_transactions),
                format_duration(self.build.execute_mempool_transactions),
                format_duration(self.build.finish),
                format_duration(self.build.total),
                format_duration(self.insert.validate_and_execute),
                format_duration(self.insert.insert_to_tree),
                format_duration(self.insert.total),
                format_duration(deliver_txs_total_time),
            )
        } else {
            // Block was received from network, only show Insert timing
            format!(
                "Produce[Insert[validateExec<{}>, insertTree<{}>, total<{}>]]",
                format_duration(self.insert.validate_and_execute),
                format_duration(self.insert.insert_to_tree),
                format_duration(self.insert.total),
            )
        }
    }
}

/// Global storage for block timing metrics
/// 
/// Uses IndexMap to maintain insertion order, allowing us to remove the oldest entries
/// when the cache exceeds the limit.
static BLOCK_TIMING_STORE: std::sync::OnceLock<Arc<Mutex<IndexMap<B256, BlockTimingMetrics>>>> =
    std::sync::OnceLock::new();

/// Initialize the global block timing store
fn get_timing_store() -> Arc<Mutex<IndexMap<B256, BlockTimingMetrics>>> {
    BLOCK_TIMING_STORE
        .get_or_init(|| Arc::new(Mutex::new(IndexMap::new())))
        .clone()
}

/// Store timing metrics for a block
/// 
/// If the block already exists, it will be updated and moved to the end (most recent).
/// When the cache exceeds 1000 entries, the oldest entries are removed.
pub fn store_block_timing(block_hash: B256, metrics: BlockTimingMetrics) {
    let store = get_timing_store();
    let mut map = store.lock().unwrap();
    
    // If the block already exists, remove it first so it can be re-inserted at the end
    // This ensures that updated blocks are treated as the most recent
    if map.contains_key(&block_hash) {
        map.shift_remove(&block_hash);
    }
    
    // Insert at the end (most recent position)
    map.insert(block_hash, metrics);
    
    // Clean up old entries to prevent memory leak (keep last 1000 blocks)
    // IndexMap maintains insertion order, so we can safely remove from the front
    const MAX_ENTRIES: usize = 1000;
    while map.len() > MAX_ENTRIES {
        // Remove the oldest entry (first in insertion order)
        map.shift_remove_index(0);
    }
}

/// Retrieve timing metrics for a block
pub fn get_block_timing(block_hash: &B256) -> Option<BlockTimingMetrics> {
    let store = get_timing_store();
    let map = store.lock().unwrap();
    map.get(block_hash).cloned()
}

/// Remove timing metrics for a block (after logging)
pub fn remove_block_timing(block_hash: &B256) {
    let store = get_timing_store();
    let mut map = store.lock().unwrap();
    map.remove(block_hash);
}


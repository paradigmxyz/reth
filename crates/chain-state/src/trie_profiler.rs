//! Diagnostic tool to measure actual TrieUpdates size per ExecutedBlock
//!
//! This module provides profiling capabilities to measure the real memory impact
//! of trie updates in X Layer blocks, helping validate performance optimizations.

use reth_metrics::{metrics::Histogram, Metrics};
use std::sync::Arc;

/// Metrics for tracking trie update sizes in ExecutedBlocks
#[derive(Metrics, Clone)]
#[metrics(scope = "chain_state.trie_profiler")]
pub struct TrieProfilerMetrics {
    /// Number of account nodes per ExecutedBlock
    pub account_nodes_per_block: Histogram,
    /// Number of storage nodes per ExecutedBlock  
    pub storage_nodes_per_block: Histogram,
    /// Total trie nodes (account + storage) per ExecutedBlock
    pub total_nodes_per_block: Histogram,
    /// Estimated memory size (bytes) of account_nodes per ExecutedBlock
    pub account_nodes_bytes_per_block: Histogram,
    /// Estimated memory size (bytes) of storage_nodes per ExecutedBlock
    pub storage_nodes_bytes_per_block: Histogram,
    /// Total estimated memory (bytes) per ExecutedBlock
    pub total_bytes_per_block: Histogram,
}

/// Statistics about a single ExecutedBlock's trie updates
#[derive(Debug, Clone)]
pub struct BlockTrieStats {
    /// Block number
    pub block_number: u64,
    /// Number of account nodes updated
    pub account_nodes_count: usize,
    /// Number of storage nodes updated
    pub storage_nodes_count: usize,
    /// Estimated memory size of account nodes (bytes)
    pub account_nodes_bytes: usize,
    /// Estimated memory size of storage nodes (bytes)
    pub storage_nodes_bytes: usize,
}

impl BlockTrieStats {
    /// Total number of trie nodes in this block
    pub const fn total_nodes(&self) -> usize {
        self.account_nodes_count + self.storage_nodes_count
    }

    /// Total estimated memory size in bytes
    pub const fn total_bytes(&self) -> usize {
        self.account_nodes_bytes + self.storage_nodes_bytes
    }

    /// Estimate memory saved per extend_ref call with Arc optimization
    /// 
    /// Before: Deep clone entire BranchNodeCompact (Vec<B256> + metadata)
    /// After: Clone Arc pointer (8 bytes)
    pub const fn memory_saved_per_extend_with_arc(&self) -> usize {
        // Each account node: ~96-128 bytes cloned before, 8 bytes after
        let account_savings = self.account_nodes_bytes.saturating_sub(self.account_nodes_count * 8);
        // Each storage node: similar savings
        let storage_savings = self.storage_nodes_bytes.saturating_sub(self.storage_nodes_count * 8);
        account_savings + storage_savings
    }
}

/// Aggregated statistics across multiple blocks
#[derive(Debug, Clone)]
pub struct AggregatedTrieStats {
    /// Number of blocks analyzed
    pub block_count: usize,
    /// Average account nodes per block
    pub avg_account_nodes: f64,
    /// Average storage nodes per block
    pub avg_storage_nodes: f64,
    /// Average total nodes per block
    pub avg_total_nodes: f64,
    /// Average memory per block (bytes)
    pub avg_bytes_per_block: f64,
    /// Estimated memory saved per 1024-block aggregation with Arc
    pub estimated_memory_saved_per_1024_blocks: usize,
    /// Min/max account nodes seen
    pub min_account_nodes: usize,
    pub max_account_nodes: usize,
    /// Min/max total nodes seen
    pub min_total_nodes: usize,
    pub max_total_nodes: usize,
}

impl AggregatedTrieStats {
    /// Calculate aggregated statistics from a collection of block stats
    pub fn from_blocks(stats: &[BlockTrieStats]) -> Self {
        if stats.is_empty() {
            return Self::default()
        }

        let block_count = stats.len();
        let total_account_nodes: usize = stats.iter().map(|s| s.account_nodes_count).sum();
        let total_storage_nodes: usize = stats.iter().map(|s| s.storage_nodes_count).sum();
        let total_nodes: usize = stats.iter().map(|s| s.total_nodes()).sum();
        let total_bytes: usize = stats.iter().map(|s| s.total_bytes()).sum();

        let avg_account_nodes = total_account_nodes as f64 / block_count as f64;
        let avg_storage_nodes = total_storage_nodes as f64 / block_count as f64;
        let avg_total_nodes = total_nodes as f64 / block_count as f64;
        let avg_bytes_per_block = total_bytes as f64 / block_count as f64;

        let min_account_nodes = stats.iter().map(|s| s.account_nodes_count).min().unwrap_or(0);
        let max_account_nodes = stats.iter().map(|s| s.account_nodes_count).max().unwrap_or(0);
        let min_total_nodes = stats.iter().map(|s| s.total_nodes()).min().unwrap_or(0);
        let max_total_nodes = stats.iter().map(|s| s.total_nodes()).max().unwrap_or(0);

        // Estimate savings for 1024-block aggregation (typical RPC call scenario)
        let estimated_memory_saved_per_1024_blocks = 
            (avg_bytes_per_block * 1024.0) as usize - (avg_total_nodes * 8.0 * 1024.0) as usize;

        Self {
            block_count,
            avg_account_nodes,
            avg_storage_nodes,
            avg_total_nodes,
            avg_bytes_per_block,
            estimated_memory_saved_per_1024_blocks,
            min_account_nodes,
            max_account_nodes,
            min_total_nodes,
            max_total_nodes,
        }
    }

    /// Format as human-readable report
    pub fn report(&self) -> String {
        format!(
            r#"
╔═══════════════════════════════════════════════════════════════╗
║         X Layer Trie Updates Profiling Results                ║
╠═══════════════════════════════════════════════════════════════╣
║ Blocks Analyzed:              {:>10}                      ║
║                                                               ║
║ Average Nodes per Block:                                      ║
║   • Account Nodes:            {:>10.1}                      ║
║   • Storage Nodes:            {:>10.1}                      ║
║   • Total Nodes:              {:>10.1}                      ║
║                                                               ║
║ Node Count Range:                                             ║
║   • Min Account Nodes:        {:>10}                      ║
║   • Max Account Nodes:        {:>10}                      ║
║   • Min Total Nodes:          {:>10}                      ║
║   • Max Total Nodes:          {:>10}                      ║
║                                                               ║
║ Memory Impact:                                                ║
║   • Avg Bytes/Block:          {:>10.1} bytes              ║
║   • Memory Saved (1024 blks): {:>10.1} MB                 ║
║                                                               ║
║ Performance Projection (Arc optimization):                    ║
║   • Before: ~{:.1} MB cloned per 1024-block aggregation       ║
║   • After:  ~{:.1} MB pointers per 1024-block aggregation     ║
║   • Reduction: {:.1}x                                          ║
╚═══════════════════════════════════════════════════════════════╝
"#,
            self.block_count,
            self.avg_account_nodes,
            self.avg_storage_nodes,
            self.avg_total_nodes,
            self.min_account_nodes,
            self.max_account_nodes,
            self.min_total_nodes,
            self.max_total_nodes,
            self.avg_bytes_per_block,
            self.estimated_memory_saved_per_1024_blocks as f64 / 1_000_000.0,
            self.avg_bytes_per_block * 1024.0 / 1_000_000.0,
            (self.avg_total_nodes * 8.0 * 1024.0) / 1_000_000.0,
            (self.avg_bytes_per_block * 1024.0) / (self.avg_total_nodes * 8.0 * 1024.0),
        )
    }
}

impl Default for AggregatedTrieStats {
    fn default() -> Self {
        Self {
            block_count: 0,
            avg_account_nodes: 0.0,
            avg_storage_nodes: 0.0,
            avg_total_nodes: 0.0,
            avg_bytes_per_block: 0.0,
            estimated_memory_saved_per_1024_blocks: 0,
            min_account_nodes: 0,
            max_account_nodes: 0,
            min_total_nodes: 0,
            max_total_nodes: 0,
        }
    }
}

/// Profiler for measuring trie update sizes in ExecutedBlocks
#[derive(Clone)]
pub struct TrieProfiler {
    metrics: TrieProfilerMetrics,
    enabled: Arc<std::sync::atomic::AtomicBool>,
}

impl TrieProfiler {
    /// Create a new profiler instance
    pub fn new() -> Self {
        Self {
            metrics: TrieProfilerMetrics::default(),
            enabled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Enable profiling
    pub fn enable(&self) {
        self.enabled.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// Disable profiling  
    pub fn disable(&self) {
        self.enabled.store(false, std::sync::atomic::Ordering::Relaxed);
    }

    /// Check if profiling is enabled
    pub fn is_enabled(&self) -> bool {
        self.enabled.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Profile a trie update and record metrics
    pub fn profile_trie_updates(
        &self,
        block_number: u64,
        trie_updates: &reth_trie::updates::TrieUpdates,
    ) -> BlockTrieStats {
        let account_nodes_count = trie_updates.account_nodes.len();
        let storage_nodes_count: usize = trie_updates.storage_tries.values().map(|st| st.storage_nodes.len()).sum();

        // Estimate memory size: BranchNodeCompact is roughly 96-128 bytes
        // We'll use 112 bytes as average (state_mask=32b, tree_mask=32b, hash_mask=32b, 
        // hashes=Vec<B256> with avg 2 entries = 64 bytes)
        const AVG_BRANCH_NODE_SIZE: usize = 112;
        let account_nodes_bytes = account_nodes_count * AVG_BRANCH_NODE_SIZE;
        let storage_nodes_bytes = storage_nodes_count * AVG_BRANCH_NODE_SIZE;

        let stats = BlockTrieStats {
            block_number,
            account_nodes_count,
            storage_nodes_count,
            account_nodes_bytes,
            storage_nodes_bytes,
        };

        // Record metrics if enabled
        if self.is_enabled() {
            self.metrics.account_nodes_per_block.record(account_nodes_count as f64);
            self.metrics.storage_nodes_per_block.record(storage_nodes_count as f64);
            self.metrics.total_nodes_per_block.record(stats.total_nodes() as f64);
            self.metrics.account_nodes_bytes_per_block.record(account_nodes_bytes as f64);
            self.metrics.storage_nodes_bytes_per_block.record(storage_nodes_bytes as f64);
            self.metrics.total_bytes_per_block.record(stats.total_bytes() as f64);
        }

        stats
    }
}

impl Default for TrieProfiler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_block_trie_stats() {
        let stats = BlockTrieStats {
            block_number: 1000,
            account_nodes_count: 100,
            storage_nodes_count: 400,
            account_nodes_bytes: 11_200,  // 100 * 112
            storage_nodes_bytes: 44_800,  // 400 * 112
        };

        assert_eq!(stats.total_nodes(), 500);
        assert_eq!(stats.total_bytes(), 56_000);
        
        // Memory saved: 56,000 bytes - (500 * 8 bytes for Arc pointers) = 52,000 bytes
        assert_eq!(stats.memory_saved_per_extend_with_arc(), 52_000);
    }

    #[test]
    fn test_aggregated_stats() {
        let blocks = vec![
            BlockTrieStats {
                block_number: 1,
                account_nodes_count: 100,
                storage_nodes_count: 400,
                account_nodes_bytes: 11_200,
                storage_nodes_bytes: 44_800,
            },
            BlockTrieStats {
                block_number: 2,
                account_nodes_count: 200,
                storage_nodes_count: 800,
                account_nodes_bytes: 22_400,
                storage_nodes_bytes: 89_600,
            },
        ];

        let agg = AggregatedTrieStats::from_blocks(&blocks);
        
        assert_eq!(agg.block_count, 2);
        assert_eq!(agg.avg_account_nodes, 150.0);
        assert_eq!(agg.avg_storage_nodes, 600.0);
        assert_eq!(agg.avg_total_nodes, 750.0);
        assert_eq!(agg.min_account_nodes, 100);
        assert_eq!(agg.max_account_nodes, 200);
    }
}

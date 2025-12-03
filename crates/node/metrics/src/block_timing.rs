//! Block timing metrics for tracking block production and execution times

use alloy_primitives::B256;
use indexmap::IndexMap;
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

pub use crate::block_timing_prometheus::BlockTimingPrometheusMetrics;

/// Timing metrics for block building phase
#[derive(Debug, Clone, Default)]
pub struct BuildTiming {
    /// Time spent applying pre-execution changes
    pub apply_pre_execution_changes: Duration,
    /// Time spent executing sequencer transactions
    pub exec_sequencer_transactions: Duration,
    /// Time spent selecting/packing mempool transactions
    pub select_mempool_transactions: Duration,
    /// Time spent executing mempool transactions
    pub exec_mempool_transactions: Duration,
    /// Time spent calculating state root
    pub calc_state_root: Duration,
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

/// Complete timing metrics for a block
#[derive(Debug, Clone, Default)]
pub struct BlockTimingMetrics {
    /// Block building phase timing
    pub build: BuildTiming,
    /// Block insertion phase timing
    pub insert: InsertTiming,
}

impl BlockTimingMetrics {
    /// Format timing metrics for logging
    pub fn format_for_log(&self) -> String {
        let format_duration = |d: Duration| {
            let secs = d.as_secs_f64();
            match () {
                _ if secs >= 1.0 => format!("{:.3}s", secs),
                _ if secs >= 0.001 => {
                    let ms = secs * 1000.0;
                    if ms.fract() == 0.0 {
                        format!("{}ms", ms as u64)
                    } else {
                        format!("{:.3}ms", ms)
                    }
                }
                _ if secs >= 0.000001 => {
                    let us = secs * 1_000_000.0;
                    if us.fract() == 0.0 {
                        format!("{}µs", us as u64)
                    } else {
                        format!("{:.3}µs", us)
                    }
                }
                _ => format!("{}ns", d.as_nanos()),
            }
        };

        // Check if block was built locally (has build timing) or received from network
        let is_locally_built = self.build.total.as_nanos() > 0;

        if is_locally_built {
            // Block was built locally, show full timing including Build
            // Note: Transaction execution times (execSeqTxs, selectMempoolTxs, execMempoolTxs) are
            // already shown in Build phase
            format!(
                "Produce[Build[applyPreExec<{}>, execSeqTxs<{}>, selectMempoolTxs<{}>, execMempoolTxs<{}>, calcStateRoot<{}>, total<{}>], Insert[validateExec<{}>, insertTree<{}>, total<{}>]]",
                format_duration(self.build.apply_pre_execution_changes),
                format_duration(self.build.exec_sequencer_transactions),
                format_duration(self.build.select_mempool_transactions),
                format_duration(self.build.exec_mempool_transactions),
                format_duration(self.build.calc_state_root),
                format_duration(self.build.total),
                format_duration(self.insert.validate_and_execute),
                format_duration(self.insert.insert_to_tree),
                format_duration(self.insert.total),
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
    BLOCK_TIMING_STORE.get_or_init(|| Arc::new(Mutex::new(IndexMap::new()))).clone()
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
    map.shift_remove(block_hash);
}

// ============================================================================
// RAII-based timing helpers
// ============================================================================

/// RAII guard that automatically records timing to a mutable reference and optionally to Prometheus
/// when dropped.
///
/// Usage:
/// ```rust
/// let mut timing = Duration::default();
/// {
///     let _guard = TimingGuard::new(&mut timing);
///     // ... do work ...
/// } // Guard is dropped here and timing is automatically recorded
/// ```
pub struct TimingGuard<'a> {
    start: Option<Instant>,
    target: &'a mut Duration,
    prometheus_histogram: Option<&'a metrics::Histogram>,
}

impl<'a> TimingGuard<'a> {
    /// Create a new timing guard that will record the elapsed time to `target` when dropped.
    pub fn new(target: &'a mut Duration) -> Self {
        Self { start: Some(Instant::now()), target, prometheus_histogram: None }
    }

    /// Create a new timing guard that records to both `target` and Prometheus histogram.
    pub fn new_with_prometheus(
        target: &'a mut Duration,
        prometheus_histogram: &'a metrics::Histogram,
    ) -> Self {
        Self {
            start: Some(Instant::now()),
            target,
            prometheus_histogram: Some(prometheus_histogram),
        }
    }

    fn record(&mut self) -> Option<Duration> {
        let start = self.start.take()?;
        let duration = start.elapsed();
        *self.target = duration;
        if let Some(histogram) = self.prometheus_histogram {
            histogram.record(duration.as_secs_f64());
        }
        Some(duration)
    }

    /// Manually finish timing and return the duration.
    pub fn finish(mut self) -> Duration {
        if let Some(start) = self.start.take() {
            let duration = start.elapsed();
            *self.target = duration;
            if let Some(histogram) = self.prometheus_histogram {
                histogram.record(duration.as_secs_f64());
            }
            duration
        } else {
            *self.target
        }
    }
}

impl<'a> Drop for TimingGuard<'a> {
    fn drop(&mut self) {
        let _ = self.record();
    }
}

/// Context for managing block timing metrics using RAII.
///
/// This provides a cleaner API for recording timing metrics throughout the block
/// building and insertion process. The context automatically stores metrics when dropped.
///
/// Supports both logging (via IndexMap) and Prometheus metrics recording.
pub struct BlockTimingContext {
    block_hash: B256,
    metrics: BlockTimingMetrics,
    auto_store: bool,
    prometheus_metrics: Option<BlockTimingPrometheusMetrics>,
}

impl BlockTimingContext {
    /// Create a new timing context for a block.
    ///
    /// If timing metrics already exist for this block (e.g., from build phase),
    /// they will be loaded. Otherwise, a new empty metrics structure is created.
    ///
    /// Metrics will be automatically stored when the context is dropped.
    pub fn new(block_hash: B256) -> Self {
        Self::with_metrics(block_hash, get_block_timing(&block_hash), None)
    }

    /// Create a new timing context for a block, initializing with empty metrics.
    ///
    /// Metrics will be automatically stored when the context is dropped.
    pub fn new_empty(block_hash: B256) -> Self {
        Self::with_metrics(block_hash, None, None)
    }

    /// Create a new timing context with Prometheus metrics support.
    ///
    /// This enables automatic recording to Prometheus in addition to logging.
    pub fn new_with_prometheus(
        block_hash: B256,
        prometheus_metrics: BlockTimingPrometheusMetrics,
    ) -> Self {
        Self::with_metrics(block_hash, get_block_timing(&block_hash), Some(prometheus_metrics))
    }

    /// Create a new empty timing context with Prometheus metrics support.
    pub fn new_empty_with_prometheus(
        block_hash: B256,
        prometheus_metrics: BlockTimingPrometheusMetrics,
    ) -> Self {
        Self::with_metrics(block_hash, None, Some(prometheus_metrics))
    }

    /// Internal helper to create a context with optional metrics and Prometheus support.
    fn with_metrics(
        block_hash: B256,
        metrics: Option<BlockTimingMetrics>,
        prometheus_metrics: Option<BlockTimingPrometheusMetrics>,
    ) -> Self {
        Self {
            block_hash,
            metrics: metrics.unwrap_or_default(),
            auto_store: true,
            prometheus_metrics,
        }
    }

    /// Get a mutable reference to the metrics.
    pub fn metrics_mut(&mut self) -> &mut BlockTimingMetrics {
        &mut self.metrics
    }

    /// Get a reference to the metrics.
    pub fn metrics(&self) -> &BlockTimingMetrics {
        &self.metrics
    }

    /// Update the block hash for this context.
    pub fn set_block_hash(&mut self, block_hash: B256) {
        self.block_hash = block_hash;
    }

    /// Create a timing guard for recording build phase: apply pre-execution changes.
    pub fn time_apply_pre_execution_changes(&mut self) -> TimingGuard<'_> {
        match &self.prometheus_metrics {
            Some(prom_metrics) => TimingGuard::new_with_prometheus(
                &mut self.metrics.build.apply_pre_execution_changes,
                &prom_metrics.build_apply_pre_execution_changes,
            ),
            None => TimingGuard::new(&mut self.metrics.build.apply_pre_execution_changes),
        }
    }

    /// Create a timing guard for recording build phase: execute sequencer transactions.
    pub fn time_exec_sequencer_transactions(&mut self) -> TimingGuard<'_> {
        match &self.prometheus_metrics {
            Some(prom_metrics) => TimingGuard::new_with_prometheus(
                &mut self.metrics.build.exec_sequencer_transactions,
                &prom_metrics.build_exec_sequencer_transactions,
            ),
            None => TimingGuard::new(&mut self.metrics.build.exec_sequencer_transactions),
        }
    }

    /// Create a timing guard for recording build phase: select/pack mempool transactions.
    pub fn time_select_mempool_transactions(&mut self) -> TimingGuard<'_> {
        match &self.prometheus_metrics {
            Some(prom_metrics) => TimingGuard::new_with_prometheus(
                &mut self.metrics.build.select_mempool_transactions,
                &prom_metrics.build_select_mempool_transactions,
            ),
            None => TimingGuard::new(&mut self.metrics.build.select_mempool_transactions),
        }
    }

    /// Create a timing guard for recording build phase: execute mempool transactions.
    pub fn time_exec_mempool_transactions(&mut self) -> TimingGuard<'_> {
        match &self.prometheus_metrics {
            Some(prom_metrics) => TimingGuard::new_with_prometheus(
                &mut self.metrics.build.exec_mempool_transactions,
                &prom_metrics.build_exec_mempool_transactions,
            ),
            None => TimingGuard::new(&mut self.metrics.build.exec_mempool_transactions),
        }
    }

    /// Create a timing guard for recording build phase: calculate state root.
    pub fn time_calc_state_root(&mut self) -> TimingGuard<'_> {
        match &self.prometheus_metrics {
            Some(prom_metrics) => TimingGuard::new_with_prometheus(
                &mut self.metrics.build.calc_state_root,
                &prom_metrics.build_calc_state_root,
            ),
            None => TimingGuard::new(&mut self.metrics.build.calc_state_root),
        }
    }

    /// Create a timing guard for recording insert phase: validate and execute.
    pub fn time_validate_and_execute(&mut self) -> TimingGuard<'_> {
        match &self.prometheus_metrics {
            Some(prom_metrics) => TimingGuard::new_with_prometheus(
                &mut self.metrics.insert.validate_and_execute,
                &prom_metrics.insert_validate_and_execute,
            ),
            None => TimingGuard::new(&mut self.metrics.insert.validate_and_execute),
        }
    }

    /// Create a timing guard for recording insert phase: insert to tree.
    pub fn time_insert_to_tree(&mut self) -> TimingGuard<'_> {
        match &self.prometheus_metrics {
            Some(prom_metrics) => TimingGuard::new_with_prometheus(
                &mut self.metrics.insert.insert_to_tree,
                &prom_metrics.insert_insert_to_tree,
            ),
            None => TimingGuard::new(&mut self.metrics.insert.insert_to_tree),
        }
    }

    /// Manually store the current metrics to the global store.
    /// This is called automatically on drop if `auto_store` is true.
    pub fn store(&self) {
        store_block_timing(self.block_hash, self.metrics.clone());
    }

    /// Set whether metrics should be automatically stored on drop.
    pub fn set_auto_store(&mut self, auto_store: bool) {
        self.auto_store = auto_store;
    }

    /// Calculate total build time from individual components.
    fn calculate_build_total(&self) -> Duration {
        self.metrics.build.apply_pre_execution_changes +
            self.metrics.build.exec_sequencer_transactions +
            self.metrics.build.select_mempool_transactions +
            self.metrics.build.exec_mempool_transactions +
            self.metrics.build.calc_state_root
    }

    /// Calculate total insert time from individual components.
    fn calculate_insert_total(&self) -> Duration {
        self.metrics.insert.validate_and_execute + self.metrics.insert.insert_to_tree
    }

    /// Calculate and update total times.
    pub fn update_totals(&mut self) {
        self.metrics.build.total = self.calculate_build_total();
        self.metrics.insert.total = self.calculate_insert_total();

        if let Some(prom_metrics) = self.prometheus_metrics.take() {
            prom_metrics.build_total.record(self.metrics.build.total.as_secs_f64());
            prom_metrics.insert_total.record(self.metrics.insert.total.as_secs_f64());
        }
    }
}

impl Drop for BlockTimingContext {
    fn drop(&mut self) {
        if self.auto_store {
            self.update_totals();
            self.store();
        }
    }
}

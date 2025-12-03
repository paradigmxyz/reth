//! Prometheus metrics for block timing

use reth_metrics::{metrics::Histogram, Metrics};

/// Prometheus metrics for block timing.
///
/// These metrics track the duration of various phases in block production and insertion.
#[derive(Metrics, Clone)]
#[metrics(scope = "block.timing")]
pub struct BlockTimingPrometheusMetrics {
    /// Time spent applying pre-execution changes during block building
    pub build_apply_pre_execution_changes: Histogram,

    /// Time spent executing sequencer transactions during block building
    pub build_exec_sequencer_transactions: Histogram,

    /// Time spent selecting/packing mempool transactions during block building
    pub build_select_mempool_transactions: Histogram,

    /// Time spent executing mempool transactions during block building
    pub build_exec_mempool_transactions: Histogram,

    /// Time spent calculating state root during block building
    pub build_calc_state_root: Histogram,

    /// Total time spent in block building phase
    pub build_total: Histogram,

    /// Time spent validating and executing the block during insertion
    pub insert_validate_and_execute: Histogram,

    /// Time spent inserting to tree state
    pub insert_insert_to_tree: Histogram,

    /// Total time spent in block insertion phase
    pub insert_total: Histogram,
}

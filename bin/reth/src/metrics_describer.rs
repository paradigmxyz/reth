//! Metrics describer

use reth_stages::stages_metrics_describer;

/// Describe all metrics.
pub(crate) fn describe() {
    stages_metrics_describer::describe();
}

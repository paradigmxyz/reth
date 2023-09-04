//! Task Executor Metrics
use reth_metrics::{metrics::Counter, Metrics};

/// Task Executor Metrics
#[derive(Metrics, Clone)]
#[metrics(scope = "executor.spawn")]
pub struct TaskExecutorMetrics {
    /// Number of spawned critical tasks
    pub(crate) critical_tasks: Counter,
    /// Number of finished spawned critical tasks
    pub(crate) finished_critical_tasks: Counter,
    /// Number of spawned regular tasks
    pub(crate) regular_tasks: Counter,
    /// Number of finished spawned regular tasks
    pub(crate) finished_regular_tasks: Counter,
}

impl TaskExecutorMetrics {
    pub(crate) fn inc_critical_tasks(&self) {
        self.critical_tasks.increment(1);
    }

    pub(crate) fn inc_regular_task(&self) {
        self.regular_tasks.increment(1);
    }

    pub(crate) fn inc_finished_critical_tasks(&self) {
        self.finished_critical_tasks.increment(1);
    }

    pub(crate) fn inc_finisehd_regular_task(&self) {
        self.finished_regular_tasks.increment(1);
    }
}

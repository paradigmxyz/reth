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

    pub(crate) fn inc_regular_tasks(&self) {
        self.regular_tasks.increment(1);
    }
}

/// Helper type for increasing counters even if a task fails.
pub struct IncCounterOnDrop(Counter);

impl IncCounterOnDrop {
    /// Create a new `IncCounterOnDrop`.
    pub fn new(counter: Counter) -> Self {
        IncCounterOnDrop(counter)
    }
}

impl Drop for IncCounterOnDrop {
    fn drop(&mut self) {
        self.0.increment(1);
    }
}

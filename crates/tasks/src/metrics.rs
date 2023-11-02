//! Task Executor Metrics
use core::fmt;

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
    /// Increments the counter for spawned critical tasks.

    pub(crate) fn inc_critical_tasks(&self) {
        self.critical_tasks.increment(1);
    }
    /// Increments the counter for spawned regular tasks.

    pub(crate) fn inc_regular_tasks(&self) {
        self.regular_tasks.increment(1);
    }
}

/// Helper type for increasing counters even if a task fails
pub struct IncCounterOnDrop(Counter);

impl fmt::Debug for IncCounterOnDrop {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("IncCounterOnDrop").finish()
    }
}

impl IncCounterOnDrop {
    /// Creates a new instance of `IncCounterOnDrop` with the given counter.
    pub fn new(counter: Counter) -> Self {
        IncCounterOnDrop(counter)
    }
}

impl Drop for IncCounterOnDrop {
    /// Increment the counter when the instance is dropped.

    fn drop(&mut self) {
        self.0.increment(1);
    }
}

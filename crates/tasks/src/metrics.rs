//! Task Executor Metrics

use core::fmt;

use reth_metrics::{
    metrics::{Counter, Histogram},
    Metrics,
};
use std::time::Duration;

/// Task Executor Metrics
#[derive(Metrics, Clone)]
#[metrics(scope = "executor.spawn")]
pub struct TaskExecutorMetrics {
    /// Number of spawned critical tasks
    pub(crate) critical_tasks_total: Counter,
    /// Number of finished spawned critical tasks
    pub(crate) finished_critical_tasks_total: Counter,
    /// Number of spawned regular tasks
    pub(crate) regular_tasks_total: Counter,
    /// Number of finished spawned regular tasks
    pub(crate) finished_regular_tasks_total: Counter,
    /// Number of spawned regular blocking tasks
    pub(crate) regular_blocking_tasks_total: Counter,
    /// Number of finished spawned regular blocking tasks
    pub(crate) finished_regular_blocking_tasks_total: Counter,
}

impl TaskExecutorMetrics {
    /// Increments the counter for spawned critical tasks.
    pub(crate) fn inc_critical_tasks(&self) {
        self.critical_tasks_total.increment(1);
    }

    /// Increments the counter for spawned regular tasks.
    pub(crate) fn inc_regular_tasks(&self) {
        self.regular_tasks_total.increment(1);
    }

    /// Increments the counter for spawned regular blocking tasks.
    pub(crate) fn inc_regular_blocking_tasks(&self) {
        self.regular_blocking_tasks_total.increment(1);
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
    pub const fn new(counter: Counter) -> Self {
        Self(counter)
    }
}

impl Drop for IncCounterOnDrop {
    /// Increment the counter when the instance is dropped.
    fn drop(&mut self) {
        self.0.increment(1);
    }
}

/// Metrics for a named worker managed by the [`crate::worker_map::WorkerMap`].
#[derive(Metrics, Clone)]
#[metrics(scope = "executor.named_worker")]
pub(crate) struct WorkerThreadMetrics {
    /// Time spent queued before a named worker starts executing the task.
    queue_wait_duration_seconds: Histogram,
    /// Time spent executing a task on a named worker.
    task_duration_seconds: Histogram,
}

impl WorkerThreadMetrics {
    /// Create metrics for the named worker.
    pub(crate) fn new(worker: &'static str) -> Self {
        Self::new_with_labels(&[("worker", worker)])
    }

    /// Record how long a task waited for this worker.
    pub(crate) fn record_queue_wait(&self, duration: Duration) {
        self.queue_wait_duration_seconds.record(duration.as_secs_f64());
    }

    /// Record how long a task executed on this worker.
    pub(crate) fn record_task_duration(&self, duration: Duration) {
        self.task_duration_seconds.record(duration.as_secs_f64());
    }
}

/// Metrics for jobs submitted through [`crate::pool::WorkerPool`] wrapper methods.
#[cfg(feature = "rayon")]
#[derive(Metrics, Clone)]
#[metrics(scope = "executor.worker_pool")]
pub(crate) struct WorkerPoolMetrics {
    /// Time spent queued before the worker pool starts executing the outer job.
    job_queue_wait_duration_seconds: Histogram,
    /// Time spent executing the outer job on the worker pool.
    job_duration_seconds: Histogram,
}

#[cfg(feature = "rayon")]
impl WorkerPoolMetrics {
    /// Create metrics for the worker pool.
    pub(crate) fn new(pool: &'static str) -> Self {
        Self::new_with_labels(&[("pool", pool)])
    }

    /// Record how long an outer job waited for this pool.
    pub(crate) fn record_job_queue_wait(&self, duration: Duration) {
        self.job_queue_wait_duration_seconds.record(duration.as_secs_f64());
    }

    /// Record how long an outer job executed on this pool.
    pub(crate) fn record_job_duration(&self, duration: Duration) {
        self.job_duration_seconds.record(duration.as_secs_f64());
    }
}

//! Explicit entry point for a blocking workload.

#[cfg(feature = "fuzz")]
use super::deterministic;

/// Runs a workload using the backend selected by the `fuzz` feature.
///
/// Runtime construction exposes no scheduler configuration. Simulation runs one
/// fixed round-robin execution; every spawned task must finish before returning.
#[derive(Clone, Copy, Debug, Default)]
pub struct Runtime;

impl Runtime {
    /// Creates an execution entry point.
    pub const fn new() -> Self {
        Self
    }

    /// Runs `work` and returns its result. Callers must join or finish spawned work.
    ///
    /// Generate Arbitrary and property-test inputs before calling `run`, then move
    /// owned inputs into `work`. Derived Arbitrary implementations can use native
    /// recursion-count thread-local state, which is not simulated.
    ///
    /// Simulated primitives must be created inside `work`, not captured from a
    /// previous execution or returned from it. Runs may not be nested. Panics
    /// propagate to the caller. Simulation rejects a workload that returns with
    /// pending timers, or whose unfinished tasks later attempt a timed operation;
    /// finish or join spawned work before `work` returns.
    pub fn run<F, T>(&self, work: F) -> T
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        #[cfg(not(feature = "fuzz"))]
        {
            work()
        }
        #[cfg(feature = "fuzz")]
        {
            deterministic::run(work)
        }
    }
}

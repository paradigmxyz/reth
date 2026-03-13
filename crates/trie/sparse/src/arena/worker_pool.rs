//! A lightweight persistent thread pool for parallel subtrie updates.
//!
//! Wraps the `threadpool` crate with a fixed number of workers (default 2),
//! avoiding rayon's fork/join overhead (~17µs/dispatch).

/// A persistent pool of worker threads for parallel subtrie processing.
pub(super) struct SubtrieWorkerPool {
    pool: threadpool::ThreadPool,
}

// SAFETY: threadpool::ThreadPool is Send but not Sync. We only use the pool
// from &mut self contexts on ArenaParallelSparseTrie (single caller).
unsafe impl Sync for SubtrieWorkerPool {}

impl SubtrieWorkerPool {
    /// Creates a new pool with `num_threads` persistent worker threads.
    pub(super) fn new(num_threads: usize) -> Self {
        Self {
            pool: threadpool::ThreadPool::with_name("subtrie-worker".into(), num_threads),
        }
    }

    /// Returns the number of worker threads in the pool.
    pub(super) fn num_workers(&self) -> usize {
        self.pool.max_count()
    }

    /// Submit a `'static + Send` closure to the pool.
    pub(super) fn execute(&self, f: impl FnOnce() + Send + 'static) {
        self.pool.execute(f);
    }

    /// Block until all submitted jobs complete.
    pub(super) fn join(&self) {
        self.pool.join();
    }
}

impl core::fmt::Debug for SubtrieWorkerPool {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SubtrieWorkerPool")
            .field("num_workers", &self.pool.max_count())
            .finish()
    }
}

//! This crate includes thread pool primitives for mixed I/O and CPU-bound workloads.
//!
//! This is intended to be used for computing the state root during payload validation.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg, doc_auto_cfg))]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

// Workflow
//
// Main thread:
//  - executes block sequentially
//  - awaits stateroot
//
// Stateroot task:
//   - spawns multiproof calculations
//     * fetches multiproofs from disk
//        * this spawns more tasks Starting proof calculation
//        * joins all
//     * (emits proof calculated message)
// Prewarming task:
//  - schedules transactions (this should be a simple loop over a channel that spawns tx execution
//    and receives result until cancelled)
//  - should be tokio blocking task, limited to
//
// 1. spawn stateroot task
// 2. spawn transaction prewarming task

use rayon::ThreadPool as RayonPool;
use std::sync::Arc;
use tokio::{runtime::Runtime, task::JoinHandle};

/// An executor for mixed I/O and CPU workloads.
#[derive(Debug, Clone)]
pub struct WorkloadExecutor {
    inner: WorkloadExecutorInner,
}

impl WorkloadExecutor {
    pub fn new(cpu_threads: usize) -> Self {
        // Create runtime for I/O operations
        let runtime = Arc::new(Runtime::new().unwrap());

        // Create Rayon thread pool for CPU work
        let rayon_pool =
            Arc::new(rayon::ThreadPoolBuilder::new().num_threads(cpu_threads).build().unwrap());

        WorkloadExecutor { inner: WorkloadExecutorInner { runtime, rayon_pool } }
    }

    /// Returns access to the tokio runtime
    pub fn runtime(&self) -> &Arc<Runtime> {
        &self.inner.runtime
    }

    /// Shorthand for [Runtime::spawn_blocking]
    #[track_caller]
    pub fn spawn_blocking<F, R>(&self, func: F) -> JoinHandle<R>
    where
        F: FnOnce() -> R + Send + 'static,
        R: Send + 'static,
    {
        self.runtime().spawn_blocking(func)
    }

    /// Returns access to the rayon pool
    pub fn rayon_pool(&self) -> &Arc<rayon::ThreadPool> {
        &self.inner.rayon_pool
    }
}

#[derive(Debug, Clone)]
struct WorkloadExecutorInner {
    // TODO: replace with main tokio handle instead or even task executor?
    runtime: Arc<Runtime>,
    rayon_pool: Arc<RayonPool>,
}

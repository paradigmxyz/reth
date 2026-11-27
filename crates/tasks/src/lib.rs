//! Reth task management.
//!
//! Blocking concurrency primitives are provided by [`reth_concurrency::sync`].
//!
//! # Feature Flags
//!
//! - `rayon`: Enable native task-management helpers for rayon thread pools.
//! - `fuzz`: Enable simulation support; primitives remain native unless `--cfg dst` is also set.
//!   With both enabled, threads, selected channels, pools, and time use the deterministic backend.
//!   Existing Tokio channels remain native and are not controlled by Shuttle.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]

// Backend-dependent task integration must agree with the selected primitive types.
const _: () = assert!(
    cfg!(all(feature = "fuzz", dst)) == reth_concurrency::sync::IS_SIMULATED,
    "enable reth-tasks/fuzz when enabling reth-concurrency/fuzz with --cfg dst"
);

#[cfg(all(feature = "fuzz", not(dst)))]
use shuttle as _;

pub mod cancel;
pub mod lazy;
pub mod metrics;
pub mod runtime;
pub mod shutdown;
pub mod utils;
pub(crate) mod worker_map;

#[cfg(feature = "rayon")]
pub mod pool;
#[cfg(feature = "rayon")]
pub use pool::{build_pool_with_panic_handler, Worker, WorkerPool, WorkerStatefulPool};

/// Lock-free ordered parallel iterator extension trait.
#[cfg(feature = "rayon")]
pub mod for_each_ordered;
#[cfg(feature = "rayon")]
pub use for_each_ordered::ForEachOrdered;

pub use cancel::{CancelOnDrop, ManualCancel};
pub use lazy::LazyHandle;
#[cfg(feature = "rayon")]
pub use runtime::RayonConfig;
pub use runtime::{Runtime, RuntimeBuildError, RuntimeBuilder, RuntimeConfig, TokioConfig};

mod task_manager;
pub use task_manager::{
    spawn_blocking, spawn_scoped_thread, spawn_thread, PanickedTaskError, TaskExecutor, TaskManager,
};
// Preserve the original native thread helper names.
#[cfg(not(all(feature = "fuzz", dst)))]
pub use task_manager::{
    spawn_scoped_thread as spawn_scoped_os_thread, spawn_thread as spawn_os_thread,
};
pub(crate) use task_manager::{wait_for_shutdown_progress, TaskEvent};

#[cfg(test)]
mod test_utils {
    use reth_concurrency::sync::Runtime;
    use tokio::runtime::Builder;

    // This lock is acquired before entering the selected runtime and serializes simulations.
    #[cfg(all(feature = "fuzz", dst))]
    #[allow(
        clippy::disallowed_types,
        reason = "Host lock serializes separate simulations before either enters Shuttle."
    )]
    use std::sync::Mutex;

    #[cfg(feature = "rayon")]
    use futures_util::FutureExt;
    #[cfg(feature = "rayon")]
    use reth_concurrency::sync::thread;
    #[cfg(feature = "rayon")]
    #[allow(
        clippy::disallowed_types,
        reason = "Host watchdog bounds stalled simulations independently of virtual time."
    )]
    use std::{
        future::Future,
        pin::pin,
        time::{Duration, Instant},
    };

    #[cfg_attr(
        all(feature = "fuzz", dst),
        allow(
            clippy::disallowed_types,
            clippy::disallowed_methods,
            reason = "Host lock serializes separate simulations before either enters Shuttle."
        )
    )]
    pub(crate) fn run(work: impl FnOnce() + Send + 'static) {
        // Separate simulations cannot concurrently lease the process-global compute pool.
        #[cfg(all(feature = "fuzz", dst))]
        static SIMULATION: Mutex<()> = Mutex::new(());
        #[cfg(all(feature = "fuzz", dst))]
        let _simulation = SIMULATION.lock().unwrap_or_else(|error| error.into_inner());

        let tokio = Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        // Logical threads share the runner's OS thread and its Tokio context.
        let _entered = tokio.enter();
        Runtime::new().run(work);
    }

    #[cfg(feature = "rayon")]
    #[allow(
        clippy::disallowed_types,
        clippy::disallowed_methods,
        reason = "Host watchdog bounds stalled simulations independently of virtual time."
    )]
    pub(crate) fn block_on<F: Future>(future: F) -> F::Output {
        let mut future = pin!(future);
        // A native watchdog bounds Tokio work even while this runnable poller stalls virtual time.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(result) = future.as_mut().now_or_never() {
                return result;
            }
            assert!(Instant::now() < deadline, "task future did not complete");
            // Tokio's block_on would park the OS thread instead of scheduling logical workers.
            thread::yield_now();
        }
    }
}

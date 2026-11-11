//! Reth task management.
//!
//! Tokio task management is available on non-WebAssembly targets. The [`executor`]
//! module and selected channel, compute, thread, and clock reexports are available
//! on all targets, including wasm.
//!
//! # Feature Flags
//!
//! - `rayon`: Enable native task-management helpers for rayon thread pools.
//! - `fuzz`: Use deterministic executor threads, channels, pools, and time.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(docsrs, feature(doc_cfg))]

#[cfg(feature = "fuzz")]
use ::rayon as _;

/// Executor primitives shared by task, engine, and trie workers.
pub mod executor;
/// Selected channel, compute, thread, and clock backends.
pub use executor::{channel, rayon, thread, time};

#[cfg(not(target_family = "wasm"))]
pub mod cancel;
#[cfg(not(target_family = "wasm"))]
pub mod lazy;
#[cfg(not(target_family = "wasm"))]
pub mod metrics;
#[cfg(not(target_family = "wasm"))]
pub mod runtime;
#[cfg(not(target_family = "wasm"))]
pub mod shutdown;
#[cfg(not(target_family = "wasm"))]
pub mod utils;
#[cfg(not(target_family = "wasm"))]
pub(crate) mod worker_map;

#[cfg(all(not(target_family = "wasm"), feature = "rayon"))]
pub mod pool;
#[cfg(all(not(target_family = "wasm"), feature = "rayon"))]
pub use pool::{build_pool_with_panic_handler, Worker, WorkerPool, WorkerStatefulPool};

/// Lock-free ordered parallel iterator extension trait.
#[cfg(all(not(target_family = "wasm"), feature = "rayon"))]
pub mod for_each_ordered;
#[cfg(all(not(target_family = "wasm"), feature = "rayon"))]
pub use for_each_ordered::ForEachOrdered;

#[cfg(not(target_family = "wasm"))]
pub use cancel::{CancelOnDrop, ManualCancel};
#[cfg(not(target_family = "wasm"))]
pub use lazy::LazyHandle;
#[cfg(all(not(target_family = "wasm"), feature = "rayon"))]
pub use runtime::RayonConfig;
#[cfg(not(target_family = "wasm"))]
pub use runtime::{Runtime, RuntimeBuildError, RuntimeBuilder, RuntimeConfig, TokioConfig};

#[cfg(not(target_family = "wasm"))]
mod task_manager;
#[cfg(not(target_family = "wasm"))]
pub use task_manager::{
    spawn_blocking, spawn_scoped_thread, spawn_thread, PanickedTaskError, TaskExecutor, TaskManager,
};
#[cfg(not(target_family = "wasm"))]
pub(crate) use task_manager::{wait_for_shutdown_progress, TaskEvent};

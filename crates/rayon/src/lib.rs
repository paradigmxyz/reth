//! Rayon compatibility wrapper.
//!
//! Re-exports parallel iterators from [`par-iter`](https://crates.io/crates/par-iter)
//! (which uses [`chili`](https://github.com/dragostis/chili) for lazy scheduling) and
//! the thread pool runtime from [`rayon-core`](https://crates.io/crates/rayon-core).
//!
//! `join` is provided by [`par-core`](https://crates.io/crates/par-core) with the chili
//! backend for lower overhead on short-lived fork-join tasks.

// Re-export parallel iterator modules from par-iter.
pub use par_iter::{
    array, collections, iter, option, prelude, range, range_inclusive, result, slice, str, string,
    vec,
};

// Re-export `join` from par-core (chili backend).
pub use par_core::join;

// Re-export thread pool runtime from rayon-core.
pub use rayon_core::{
    scope, scope_fifo, spawn, spawn_fifo, ThreadPool, ThreadPoolBuildError, ThreadPoolBuilder,
};

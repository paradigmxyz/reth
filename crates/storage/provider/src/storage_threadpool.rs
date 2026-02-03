//! Dedicated thread pool for storage I/O operations.
//!
//! This module provides a static rayon thread pool used for parallel writes to static files,
//! `RocksDB`, and other storage backends during block persistence.

use rayon::{ThreadPool, ThreadPoolBuilder};
use std::sync::LazyLock;

/// Number of threads in the storage I/O thread pool.
const STORAGE_POOL_THREADS: usize = 8;

/// Static thread pool for storage I/O operations.
///
/// This pool is used by [`save_blocks`](crate::DatabaseProvider::save_blocks) and related
/// methods to parallelize writes to different storage backends (static files, `RocksDB`).
pub(crate) static STORAGE_POOL: LazyLock<ThreadPool> = LazyLock::new(|| {
    ThreadPoolBuilder::new()
        .num_threads(STORAGE_POOL_THREADS)
        .thread_name(|idx| format!("reth-storage-{idx}"))
        .build()
        .expect("failed to create storage thread pool")
});

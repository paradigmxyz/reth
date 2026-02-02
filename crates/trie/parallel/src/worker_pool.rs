//! Dedicated proof worker pool utilities.
//!
//! This module provides configuration types for proof worker pools that use
//! dedicated OS threads instead of Tokio's blocking pool, reducing spawn overhead.
//!
//! # Usage
//!
//! Use [`ProofWorkerHandle::new_with_dedicated_threads`](crate::proof_task::ProofWorkerHandle::new_with_dedicated_threads)
//! to spawn workers on dedicated OS threads with named threads for debugging.

/// Configuration for a proof worker pool.
///
/// This struct holds the sizing parameters for storage and account workers.
#[derive(Debug, Clone, Copy)]
pub struct ProofWorkerPoolConfig {
    /// Number of storage workers in the pool.
    pub storage_worker_count: usize,
    /// Number of account workers in the pool.
    pub account_worker_count: usize,
}

impl ProofWorkerPoolConfig {
    /// Creates a new proof worker pool configuration.
    pub const fn new(storage_worker_count: usize, account_worker_count: usize) -> Self {
        Self { storage_worker_count, account_worker_count }
    }
}

impl Default for ProofWorkerPoolConfig {
    fn default() -> Self {
        Self::new(4, 2)
    }
}

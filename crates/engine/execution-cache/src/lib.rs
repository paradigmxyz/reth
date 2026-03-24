//! Cross-block execution cache for payload processing.
//!
//! This crate provides [`PayloadExecutionCache`], a thread-safe wrapper around a cached execution
//! state that can be shared between the engine and payload builder. It ensures safe concurrent
//! access via reference-counted availability tracking and a write-guarded update mechanism.

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

use alloy_primitives::B256;
use metrics::{Counter, Histogram};
use parking_lot::RwLock;
use reth_metrics::Metrics;
use reth_primitives_traits::FastInstant as Instant;
use std::{sync::Arc, time::Duration};
use tracing::{debug, instrument, warn};

/// Trait for cached execution state that can be stored in [`PayloadExecutionCache`].
///
/// Implementations of this trait represent a snapshot of execution state (e.g., account and storage
/// caches) associated with a specific block hash. The cache tracks availability via reference
/// counting to prevent concurrent modification.
///
/// # Required Invariants
///
/// - **`Clone` must be cheap and share underlying state.** Cloning must return another handle to
///   the same cache state — not a deep copy. This is typically achieved via `Arc`-backed internals.
/// - **Availability tracking must be shared across clones.** All clones produced by `Clone` must
///   participate in the same usage tracking. When any clone exists, [`is_available`] must return
///   `false` on the original instance. Dropping all clones must make the cache available again.
/// - **`clear_with_hash` must fully invalidate stale state.** After calling `clear_with_hash`, the
///   cache must report the new hash and contain no data from the previous block.
///
/// [`is_available`]: CachedExecution::is_available
pub trait CachedExecution: Clone + Send + Sync + std::fmt::Debug + 'static {
    /// Returns the block hash this cache was built for.
    fn executed_block_hash(&self) -> B256;

    /// Returns `true` if no other tasks currently hold a reference to this cache.
    ///
    /// A cache is "available" when only the original holder exists (no checked-out clones).
    fn is_available(&self) -> bool;

    /// Returns the current reference count of the cache's usage guard.
    fn usage_count(&self) -> usize;

    /// Clears cached state and updates the associated block hash.
    fn clear_with_hash(&mut self, hash: B256);
}

/// A guarded, thread-safe cache of execution state that tracks the most recent block's caches.
///
/// This is the cross-block cache used to accelerate sequential payload processing.
/// When a new block arrives, its parent's cached state can be reused to avoid
/// redundant database lookups.
///
/// This process assumes that payloads are received sequentially.
///
/// ## Cache Safety
///
/// **CRITICAL**: Cache update operations require exclusive access. All concurrent cache users
/// (such as prewarming tasks) must be terminated before calling
/// [`PayloadExecutionCache::update_with_guard`], otherwise the cache may be corrupted or cleared.
///
/// ## Cache vs Prewarming Distinction
///
/// **[`PayloadExecutionCache`]**:
/// - Stores parent block's execution state after completion
/// - Used to fetch parent data for next block's execution
/// - Must be exclusively accessed during save operations
///
/// **Prewarming tasks**:
/// - Speculatively load accounts/storage that might be used in transaction execution
/// - Prepare data for state root proof computation
/// - Run concurrently but must not interfere with cache saves
#[derive(Clone, Debug)]
pub struct PayloadExecutionCache<C: CachedExecution> {
    /// Guarded cloneable cache identified by a block hash.
    inner: Arc<RwLock<Option<C>>>,
    /// Metrics for cache operations.
    metrics: ExecutionCacheMetrics,
}

impl<C: CachedExecution> Default for PayloadExecutionCache<C> {
    fn default() -> Self {
        Self { inner: Arc::new(RwLock::new(None)), metrics: Default::default() }
    }
}

impl<C: CachedExecution> PayloadExecutionCache<C> {
    /// Returns the cache for `parent_hash` if it's available for use.
    ///
    /// A cache is considered available when:
    /// - It exists and matches the requested parent hash
    /// - No other tasks are currently using it (checked via Arc reference count)
    #[instrument(level = "debug", target = "engine::tree::payload_processor", skip(self))]
    pub fn get_cache_for(&self, parent_hash: B256) -> Option<C> {
        let start = Instant::now();
        let mut cache = self.inner.write();

        let elapsed = start.elapsed();
        self.metrics.execution_cache_wait_duration.record(elapsed.as_secs_f64());
        if elapsed.as_millis() > 5 {
            warn!(blocked_for=?elapsed, "Blocked waiting for execution cache mutex");
        }

        if let Some(c) = cache.as_mut() {
            let cached_hash = c.executed_block_hash();
            // Check that the cache hash matches the parent hash of the current block. It won't
            // match in case it's a fork block.
            let hash_matches = cached_hash == parent_hash;
            // Check `is_available()` to ensure no other tasks (e.g., prewarming) currently hold
            // a reference to this cache. We can only reuse it when we have exclusive access.
            let available = c.is_available();
            let usage_count = c.usage_count();

            debug!(
                target: "engine::caching",
                %cached_hash,
                %parent_hash,
                hash_matches,
                available,
                usage_count,
                "Existing cache found"
            );

            if available {
                if !hash_matches {
                    // Fork block: clear and update the hash on the ORIGINAL before cloning.
                    // This prevents the canonical chain from matching on the stale hash
                    // and picking up polluted data if the fork block fails.
                    c.clear_with_hash(parent_hash);
                }
                return Some(c.clone())
            } else if hash_matches {
                self.metrics.execution_cache_in_use.increment(1);
            }
        } else {
            debug!(target: "engine::caching", %parent_hash, "No cache found");
        }

        None
    }

    /// Waits until the execution cache becomes available for use.
    ///
    /// This acquires a write lock to ensure exclusive access, then immediately releases it.
    /// This is useful for synchronization before starting payload processing.
    ///
    /// Returns the time spent waiting for the lock.
    pub fn wait_for_availability(&self) -> Duration {
        let start = Instant::now();
        // Acquire write lock to wait for any current holders to finish
        let _guard = self.inner.write();
        let elapsed = start.elapsed();
        if elapsed.as_millis() > 5 {
            debug!(
                target: "engine::tree::payload_processor",
                blocked_for=?elapsed,
                "Waited for execution cache to become available"
            );
        }
        elapsed
    }

    /// Updates the cache with a closure that has exclusive access to the guard.
    /// This ensures that all cache operations happen atomically.
    ///
    /// ## CRITICAL SAFETY REQUIREMENT
    ///
    /// **Before calling this method, you MUST ensure there are no other active cache users.**
    /// This includes:
    /// - No running prewarming task instances that could write to the cache
    /// - No concurrent transactions that might access the cached state
    /// - All prewarming operations must be completed or cancelled
    ///
    /// Violating this requirement can result in cache corruption, incorrect state data,
    /// and potential consensus failures.
    pub fn update_with_guard<F>(&self, update_fn: F)
    where
        F: FnOnce(&mut Option<C>),
    {
        let mut guard = self.inner.write();
        update_fn(&mut guard);
    }
}

/// Metrics for execution cache operations.
#[derive(Metrics, Clone)]
#[metrics(scope = "consensus.engine.beacon")]
pub struct ExecutionCacheMetrics {
    /// Counter for when the execution cache was unavailable because other threads
    /// (e.g., prewarming) are still using it.
    pub execution_cache_in_use: Counter,
    /// Time spent waiting for execution cache mutex to become available.
    pub execution_cache_wait_duration: Histogram,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal test implementation of [`CachedExecution`] that tracks availability via
    /// `Arc` reference counting.
    #[derive(Debug, Clone)]
    struct MockCache {
        hash: B256,
        guard: Arc<()>,
    }

    impl MockCache {
        fn new(hash: B256) -> Self {
            Self { hash, guard: Arc::new(()) }
        }
    }

    impl CachedExecution for MockCache {
        fn executed_block_hash(&self) -> B256 {
            self.hash
        }

        fn is_available(&self) -> bool {
            Arc::strong_count(&self.guard) == 1
        }

        fn usage_count(&self) -> usize {
            Arc::strong_count(&self.guard)
        }

        fn clear_with_hash(&mut self, hash: B256) {
            self.hash = hash;
        }
    }

    #[test]
    fn single_checkout_blocks_second() {
        let cache = PayloadExecutionCache::<MockCache>::default();
        let hash = B256::from([1u8; 32]);

        cache.update_with_guard(|slot| *slot = Some(MockCache::new(hash)));

        let first = cache.get_cache_for(hash);
        assert!(first.is_some());

        // Second checkout fails because first clone is still alive
        let second = cache.get_cache_for(hash);
        assert!(second.is_none());
    }

    #[test]
    fn checkout_available_after_drop() {
        let cache = PayloadExecutionCache::<MockCache>::default();
        let hash = B256::from([2u8; 32]);

        cache.update_with_guard(|slot| *slot = Some(MockCache::new(hash)));

        let checked_out = cache.get_cache_for(hash);
        assert!(checked_out.is_some());
        drop(checked_out);

        // Now available again
        let second = cache.get_cache_for(hash);
        assert!(second.is_some());
    }

    #[test]
    fn hash_mismatch_clears_and_retags() {
        let cache = PayloadExecutionCache::<MockCache>::default();
        let hash_a = B256::from([0xAA; 32]);
        let hash_b = B256::from([0xBB; 32]);

        cache.update_with_guard(|slot| *slot = Some(MockCache::new(hash_a)));

        // Request for a different hash clears and retags
        let checked_out = cache.get_cache_for(hash_b);
        assert!(checked_out.is_some());
        assert_eq!(checked_out.unwrap().executed_block_hash(), hash_b);
    }

    #[test]
    fn empty_cache_returns_none() {
        let cache = PayloadExecutionCache::<MockCache>::default();
        assert!(cache.get_cache_for(B256::ZERO).is_none());
    }
}

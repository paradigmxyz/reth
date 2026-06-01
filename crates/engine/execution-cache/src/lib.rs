//! Cross-block execution cache for payload processing.
//!
//! This crate provides the core caching infrastructure used during block execution:
//! - [`ExecutionCache`]: Fixed-size concurrent caches for accounts, storage, and bytecode
//! - [`SavedCache`]: An execution cache snapshot associated with a specific block hash
//! - [`PayloadExecutionCache`]: Thread-safe wrapper for sharing cached state across payload
//!   processing tasks

#![doc(
    html_logo_url = "https://raw.githubusercontent.com/paradigmxyz/reth/main/assets/reth-docs.png",
    html_favicon_url = "https://avatars0.githubusercontent.com/u/97369466?s=256",
    issue_tracker_base_url = "https://github.com/paradigmxyz/reth/issues/"
)]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]

mod cached_state;
pub use cached_state::*;

use alloy_primitives::B256;
use metrics::{Counter, Histogram};
use parking_lot::Mutex;
use reth_metrics::Metrics;
use reth_primitives_traits::FastInstant as Instant;
use std::{
    sync::{mpsc, Arc},
    thread,
    time::Duration,
};
use tracing::{debug, instrument, warn};

/// A guarded, thread-safe cache of execution state that tracks the most recent block's caches.
///
/// This is the cross-block cache used to accelerate sequential payload processing.
/// When a new block arrives, its parent's cached state can be reused to avoid
/// redundant database lookups.
///
/// This process assumes that payloads are received sequentially on the canonical path, while still
/// allowing opt-in speculative builders to hold lookup leases on the current generation.
///
/// ## Cache Safety
///
/// Publication of a new cache generation must not replace the shared slot until old-generation
/// builders have released their leases. Use [`PayloadExecutionCache::publish_cache_update`] for
/// post-execution cache updates that depend on parent validity.
#[derive(Clone, Debug, Default)]
pub struct PayloadExecutionCache {
    /// Guarded cloneable cache identified by a block hash.
    inner: Arc<Mutex<PayloadExecutionCacheState>>,
    /// Metrics for cache operations.
    metrics: PayloadExecutionCacheMetrics,
}

/// Shared execution cache state.
#[derive(Debug, Default)]
struct PayloadExecutionCacheState {
    /// Currently published cache generation.
    current: Option<SavedCache>,
    /// Next generation id to assign through publication.
    next_generation: u64,
}

impl PayloadExecutionCache {
    /// Returns the cache for `parent_hash` if it matches the current generation.
    ///
    /// Matching checkouts are allowed to overlap. Callers that mutate a checked-out cache must
    /// publish a new generation rather than replacing or retagging the current one in place.
    #[instrument(level = "debug", target = "engine::tree::payload_processor", skip(self))]
    pub fn get_cache_for(&self, parent_hash: B256) -> Option<SavedCache> {
        let start = Instant::now();
        let cache = self.inner.lock();

        let elapsed = start.elapsed();
        self.metrics.execution_cache_wait_duration.record(elapsed.as_secs_f64());
        if elapsed.as_millis() > 5 {
            warn!(blocked_for=?elapsed, "Blocked waiting for execution cache mutex");
        }

        if let Some(c) = cache.current.as_ref() {
            let cached_hash = c.executed_block_hash();
            // Check that the cache hash matches the parent hash of the current block. It won't
            // match in case it's a fork block.
            let hash_matches = cached_hash == parent_hash;
            let usage_count = c.usage_count();
            let generation = c.generation();

            debug!(
                target: "engine::caching",
                %cached_hash,
                %parent_hash,
                hash_matches,
                usage_count,
                generation,
                "Existing cache found"
            );

            if hash_matches {
                return Some(c.clone())
            }

            self.metrics.execution_cache_hash_mismatch.increment(1);
        } else {
            debug!(target: "engine::caching", %parent_hash, "No cache found");
        }

        None
    }

    /// Waits until the execution cache becomes available for use.
    ///
    /// This waits for the current generation to have no checked-out leases.
    ///
    /// Returns the time spent waiting for the lock.
    pub fn wait_for_availability(&self) -> Duration {
        let start = Instant::now();
        loop {
            let tracker = self.inner.lock().current.as_ref().map(SavedCache::lease_tracker);
            let Some(tracker) = tracker else { break };
            if !tracker.has_active_leases() {
                break
            }
            self.metrics.execution_cache_in_use.increment(1);
            thread::sleep(Duration::from_millis(1));
        }
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
        F: FnOnce(&mut Option<SavedCache>),
    {
        let mut guard = self.inner.lock();
        update_fn(&mut guard.current);
    }

    /// Publishes `new_cache` as the current cache generation once publication is safe.
    ///
    /// If `valid_block_rx` is supplied, the cache is published only after parent/block validity is
    /// signaled. If the sender is dropped, the new generation is discarded. If
    /// `previous_generation` is supplied, publication waits until all leases on that previous
    /// generation have been released.
    pub fn publish_cache_update(
        &self,
        mut new_cache: SavedCache,
        previous_generation: Option<CacheLeaseTracker>,
        valid_block_rx: Option<mpsc::Receiver<()>>,
    ) -> CachePublicationOutcome {
        if let Some(valid_block_rx) = valid_block_rx &&
            valid_block_rx.recv().is_err()
        {
            debug!(target: "engine::caching", "discarded execution cache update for invalid block");
            return CachePublicationOutcome::DiscardedInvalid
        }

        while previous_generation.as_ref().is_some_and(CacheLeaseTracker::has_active_leases) {
            self.metrics.execution_cache_in_use.increment(1);
            thread::sleep(Duration::from_millis(1));
        }

        let mut guard = self.inner.lock();
        let generation = guard.next_generation;
        guard.next_generation = guard.next_generation.saturating_add(1);
        new_cache.set_generation(generation);
        guard.current = Some(new_cache);

        CachePublicationOutcome::Published
    }
}

/// Result of publishing a cache generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CachePublicationOutcome {
    /// The new cache generation was published.
    Published,
    /// The validity dependency failed before publication.
    DiscardedInvalid,
}

/// Metrics for [`PayloadExecutionCache`] operations.
#[derive(Metrics, Clone)]
#[metrics(scope = "consensus.engine.beacon")]
struct PayloadExecutionCacheMetrics {
    /// Counter for when the execution cache was unavailable because other threads
    /// (e.g., prewarming) are still using it.
    execution_cache_in_use: Counter,
    /// Counter for when the current cache generation did not match the requested parent hash.
    execution_cache_hash_mismatch: Counter,
    /// Time spent waiting for execution cache mutex to become available.
    execution_cache_wait_duration: Histogram,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching_hash_allows_overlapping_leases() {
        let cache = PayloadExecutionCache::default();
        let hash = B256::from([1u8; 32]);

        cache.update_with_guard(|slot| {
            *slot = Some(SavedCache::new(hash, ExecutionCache::new(1_000)))
        });

        let first = cache.get_cache_for(hash);
        assert!(first.is_some());

        let second = cache.get_cache_for(hash);
        assert!(second.is_some());
    }

    #[test]
    fn checkout_available_after_drop() {
        let cache = PayloadExecutionCache::default();
        let hash = B256::from([2u8; 32]);

        cache.update_with_guard(|slot| {
            *slot = Some(SavedCache::new(hash, ExecutionCache::new(1_000)))
        });

        let checked_out = cache.get_cache_for(hash);
        assert!(checked_out.is_some());
        drop(checked_out);

        let second = cache.get_cache_for(hash);
        assert!(second.is_some());
    }

    #[test]
    fn hash_mismatch_returns_none_and_preserves_current_generation() {
        let cache = PayloadExecutionCache::default();
        let hash_a = B256::from([0xAA; 32]);
        let hash_b = B256::from([0xBB; 32]);

        cache.update_with_guard(|slot| {
            *slot = Some(SavedCache::new(hash_a, ExecutionCache::new(1_000)))
        });

        let checked_out = cache.get_cache_for(hash_b);
        assert!(checked_out.is_none());

        let original = cache.get_cache_for(hash_a).expect("original generation should remain");
        assert_eq!(original.executed_block_hash(), hash_a);
    }

    #[test]
    fn empty_cache_returns_none() {
        let cache = PayloadExecutionCache::default();
        assert!(cache.get_cache_for(B256::ZERO).is_none());
    }

    #[test]
    fn publish_waits_for_previous_generation_leases() {
        let cache = PayloadExecutionCache::default();
        let old_hash = B256::from([0x11; 32]);
        let new_hash = B256::from([0x22; 32]);

        cache.update_with_guard(|slot| {
            *slot = Some(SavedCache::new(old_hash, ExecutionCache::new(1_000)))
        });

        let old_lease = cache.get_cache_for(old_hash).expect("old generation should exist");
        let tracker = old_lease.lease_tracker();
        let new_cache = SavedCache::new(new_hash, old_lease.cache().new_like());

        let cache_for_publish = cache.clone();
        let (published_tx, published_rx) = mpsc::channel();
        let publish = thread::spawn(move || {
            let outcome = cache_for_publish.publish_cache_update(new_cache, Some(tracker), None);
            published_tx.send(outcome).unwrap();
        });

        thread::sleep(Duration::from_millis(10));
        assert!(published_rx.try_recv().is_err(), "publish should wait for active lease");

        drop(old_lease);
        assert_eq!(published_rx.recv().unwrap(), CachePublicationOutcome::Published);
        publish.join().unwrap();

        let new_lease = cache.get_cache_for(new_hash).expect("new generation should publish");
        assert_eq!(new_lease.executed_block_hash(), new_hash);
    }

    #[test]
    fn publish_discards_when_validity_dependency_fails() {
        let cache = PayloadExecutionCache::default();
        let old_hash = B256::from([0x33; 32]);
        let new_hash = B256::from([0x44; 32]);

        cache.update_with_guard(|slot| {
            *slot = Some(SavedCache::new(old_hash, ExecutionCache::new(1_000)))
        });

        let new_cache = SavedCache::new(new_hash, ExecutionCache::new(1_000));
        let (valid_tx, valid_rx) = mpsc::channel();
        drop(valid_tx);

        let outcome = cache.publish_cache_update(new_cache, None, Some(valid_rx));
        assert_eq!(outcome, CachePublicationOutcome::DiscardedInvalid);

        assert!(cache.get_cache_for(new_hash).is_none());
        assert!(cache.get_cache_for(old_hash).is_some());
    }
}

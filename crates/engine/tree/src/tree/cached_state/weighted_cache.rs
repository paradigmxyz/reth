//! A weighted cache wrapper around papaya's concurrent HashMap.
//!
//! This module provides weight-based eviction on top of papaya's lock-free concurrent hash table.
//! Eviction is triggered manually (not on insert) and uses a FIFO policy based on insertion order.

use alloy_primitives::map::DefaultHashBuilder;
use papaya::HashMap as PapayaMap;
use std::{
    collections::VecDeque,
    fmt,
    hash::{BuildHasher, Hash},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

/// Inner data for the weighted cache, wrapped in Arc for cheap cloning.
struct WeightedCacheInner<K, V, S> {
    /// The underlying concurrent map.
    map: PapayaMap<K, V, S>,

    /// Maximum allowed total weight.
    max_weight: u64,

    /// Current approximate total weight of all entries.
    current_weight: AtomicUsize,

    /// Number of entries in the cache.
    entry_count: AtomicUsize,

    /// Function to compute weight for a key-value pair.
    weigher: fn(&K, &V) -> u32,

    /// Keys in approximate insertion order for FIFO eviction.
    eviction_queue: Mutex<VecDeque<K>>,
}

/// A concurrent cache with weight-based capacity limits.
///
/// This wraps papaya's lock-free HashMap and adds:
/// - Weight tracking via a custom weigher function
/// - Manual FIFO eviction when over capacity
///
/// Unlike mini_moka, eviction is NOT automatic on insert. Call [`evict_to_capacity`] manually
/// (e.g., when saving the cache) to enforce the weight limit.
///
/// This type is cheaply cloneable (just an Arc clone).
pub(crate) struct WeightedCache<K, V, S = DefaultHashBuilder> {
    inner: Arc<WeightedCacheInner<K, V, S>>,
}

impl<K, V, S> Clone for WeightedCache<K, V, S> {
    fn clone(&self) -> Self {
        Self { inner: Arc::clone(&self.inner) }
    }
}

impl<K, V, S> fmt::Debug for WeightedCache<K, V, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WeightedCache")
            .field("max_weight", &self.inner.max_weight)
            .field("current_weight", &self.inner.current_weight.load(Ordering::Relaxed))
            .field("entry_count", &self.inner.entry_count.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl<K, V, S> WeightedCache<K, V, S>
where
    K: Clone + Eq + Hash + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
    S: BuildHasher + Clone + Default + Send + Sync + 'static,
{
    /// Creates a new weighted cache with the given capacity and weigher.
    ///
    /// # Arguments
    /// * `max_weight` - Maximum total weight before eviction is needed
    /// * `weigher` - Function to compute weight for each key-value pair
    pub(crate) fn new(max_weight: u64, weigher: fn(&K, &V) -> u32) -> Self {
        Self {
            inner: Arc::new(WeightedCacheInner {
                map: PapayaMap::with_hasher(S::default()),
                max_weight,
                current_weight: AtomicUsize::new(0),
                entry_count: AtomicUsize::new(0),
                weigher,
                eviction_queue: Mutex::new(VecDeque::new()),
            }),
        }
    }

    /// Returns a value from the cache if present.
    pub(crate) fn get(&self, key: &K) -> Option<V> {
        let guard = self.inner.map.pin();
        guard.get(key).cloned()
    }

    /// Returns the value for the key if present, otherwise inserts the value returned by `f`
    /// and returns it.
    ///
    /// Returns a tuple of (value, was_inserted) where `was_inserted` is true if the value was
    /// newly inserted. Uses a single map operation.
    pub(crate) fn get_or_insert_with<F>(&self, key: K, f: F) -> (V, bool)
    where
        F: FnOnce() -> V,
    {
        let guard = self.inner.map.pin();
        let mut was_inserted = false;

        // Closure only runs if key is absent - use this to detect insertion
        let result = guard.get_or_insert_with(key.clone(), || {
            was_inserted = true;
            f()
        });

        if was_inserted {
            let new_weight = (self.inner.weigher)(&key, result) as usize;
            self.inner.current_weight.fetch_add(new_weight, Ordering::Relaxed);
            self.inner.entry_count.fetch_add(1, Ordering::Relaxed);

            if let Ok(mut queue) = self.inner.eviction_queue.lock() {
                queue.push_back(key);
            }
        }

        (result.clone(), was_inserted)
    }

    /// Inserts a key-value pair into the cache.
    ///
    /// Updates weight tracking but does NOT trigger eviction.
    /// Call [`evict_to_capacity`] separately to enforce weight limits.
    /// Uses a single map operation.
    pub(crate) fn insert(&self, key: K, value: V) {
        let guard = self.inner.map.pin();
        let new_weight = (self.inner.weigher)(&key, &value) as usize;

        // Insert returns None if new, Some(&old_value) if replaced
        match guard.insert(key.clone(), value) {
            None => {
                // New entry
                self.inner.current_weight.fetch_add(new_weight, Ordering::Relaxed);
                self.inner.entry_count.fetch_add(1, Ordering::Relaxed);

                if let Ok(mut queue) = self.inner.eviction_queue.lock() {
                    queue.push_back(key);
                }
            }
            Some(old_value) => {
                // Replaced existing entry - adjust weight difference
                let old_weight = (self.inner.weigher)(&key, old_value) as usize;
                if new_weight >= old_weight {
                    self.inner.current_weight.fetch_add(new_weight - old_weight, Ordering::Relaxed);
                } else {
                    self.inner.current_weight.fetch_sub(old_weight - new_weight, Ordering::Relaxed);
                }
            }
        }
    }

    /// Removes a key from the cache.
    pub(crate) fn remove(&self, key: &K) {
        let guard = self.inner.map.pin();
        if let Some(value) = guard.remove(key) {
            let weight = (self.inner.weigher)(key, value) as usize;
            self.inner.current_weight.fetch_sub(weight, Ordering::Relaxed);
            self.inner.entry_count.fetch_sub(1, Ordering::Relaxed);
        }
    }

    /// Returns the number of entries in the cache.
    pub(crate) fn entry_count(&self) -> u64 {
        self.inner.entry_count.load(Ordering::Relaxed) as u64
    }

    /// Adds the given delta to the current weight.
    ///
    /// Use this to update weight when the value's size changes in-place without re-inserting.
    /// For example, when adding slots to an `AccountStorageCache` that's already in the cache.
    pub(crate) fn add_weight(&self, delta: usize) {
        self.inner.current_weight.fetch_add(delta, Ordering::Relaxed);
    }

    /// Returns the current approximate weight of all entries.
    #[cfg(test)]
    pub(crate) fn weighted_size(&self) -> u64 {
        self.inner.current_weight.load(Ordering::Relaxed) as u64
    }

    /// Returns a vector of all cache entries.
    ///
    /// Note: This collects all entries into a Vec for safe iteration.
    pub(crate) fn iter(&self) -> Vec<(K, V)> {
        let guard = self.inner.map.pin();
        guard.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }

    /// Evicts entries until the cache is at or below capacity.
    ///
    /// Uses FIFO eviction based on insertion order. This should be called manually
    /// (e.g., when saving the cache) rather than on every insert.
    pub(crate) fn evict_to_capacity(&self) {
        let guard = self.inner.map.pin();
        let mut queue = match self.inner.eviction_queue.lock() {
            Ok(q) => q,
            Err(_) => return,
        };

        loop {
            let current = self.inner.current_weight.load(Ordering::Relaxed) as u64;
            if current <= self.inner.max_weight {
                break;
            }

            // Pop the oldest key from the eviction queue
            let key = {
                match queue.pop_front() {
                    Some(k) => k,
                    None => break,
                }
            };

            // Remove from the map and update weight
            if let Some(value) = guard.remove(&key) {
                let weight = (self.inner.weigher)(&key, value) as usize;
                self.inner.current_weight.fetch_sub(weight, Ordering::Relaxed);
                self.inner.entry_count.fetch_sub(1, Ordering::Relaxed);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_operations() {
        let cache: WeightedCache<u32, u32> = WeightedCache::new(100, |_k, v| *v);

        cache.insert(1, 10);
        cache.insert(2, 20);
        cache.insert(3, 30);

        assert_eq!(cache.get(&1), Some(10));
        assert_eq!(cache.get(&2), Some(20));
        assert_eq!(cache.get(&3), Some(30));
        assert_eq!(cache.get(&4), None);

        assert_eq!(cache.entry_count(), 3);
        assert_eq!(cache.weighted_size(), 60);
    }

    #[test]
    fn test_update_weight() {
        let cache: WeightedCache<u32, u32> = WeightedCache::new(100, |_k, v| *v);

        cache.insert(1, 10);
        assert_eq!(cache.weighted_size(), 10);

        // Update with larger value
        cache.insert(1, 25);
        assert_eq!(cache.weighted_size(), 25);
        assert_eq!(cache.entry_count(), 1);

        // Update with smaller value
        cache.insert(1, 5);
        assert_eq!(cache.weighted_size(), 5);
        assert_eq!(cache.entry_count(), 1);
    }

    #[test]
    fn test_eviction() {
        let cache: WeightedCache<u32, u32> = WeightedCache::new(50, |_k, v| *v);

        // Insert entries totaling 60 weight (over capacity of 50)
        cache.insert(1, 10);
        cache.insert(2, 20);
        cache.insert(3, 30);

        assert_eq!(cache.weighted_size(), 60);
        assert_eq!(cache.entry_count(), 3);

        // Evict to capacity
        cache.evict_to_capacity();

        // Should have evicted key 1 (first inserted, FIFO)
        assert!(cache.weighted_size() <= 50);
        assert_eq!(cache.get(&1), None);
        assert_eq!(cache.get(&2), Some(20));
        assert_eq!(cache.get(&3), Some(30));
    }

    #[test]
    fn test_remove() {
        let cache: WeightedCache<u32, u32> = WeightedCache::new(100, |_k, v| *v);

        cache.insert(1, 10);
        cache.insert(2, 20);

        assert_eq!(cache.weighted_size(), 30);
        assert_eq!(cache.entry_count(), 2);

        cache.remove(&1);

        assert_eq!(cache.weighted_size(), 20);
        assert_eq!(cache.entry_count(), 1);
        assert_eq!(cache.get(&1), None);
    }
}

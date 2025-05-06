//! Metered cache, which also provides storage for senders in order to queue queries that result in
//! a cache miss.

use super::metrics::CacheMetrics;
use reth_primitives_traits::InMemorySize;
use schnellru::{ByLength, Limiter, LruMap};
use std::{
    collections::{hash_map::Entry, HashMap},
    fmt::{self, Debug, Formatter},
    hash::Hash,
};

/// A multi-consumer LRU cache.
pub struct MultiConsumerLruCache<K, V, L, S>
where
    K: Hash + Eq,
    L: Limiter<K, V>,
{
    /// The LRU cache.
    cache: LruMap<K, V, L>,
    /// All queued consumers.
    queued: HashMap<K, Vec<S>>,
    /// Cache metrics
    metrics: CacheMetrics,
    // Tracked heap usage
    memory_usage: usize,
}

impl<K, V, L, S> Debug for MultiConsumerLruCache<K, V, L, S>
where
    K: Hash + Eq,
    L: Limiter<K, V>,
{
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("MultiConsumerLruCache")
            .field("cache_length", &self.cache.len())
            .field("cache_memory_usage", &self.cache.memory_usage())
            .field("queued_length", &self.queued.len())
            .field("memory_usage", &self.memory_usage)
            .finish()
    }
}

impl<K, V, L, S> MultiConsumerLruCache<K, V, L, S>
where
    K: Hash + Eq + Debug,
    L: Limiter<K, V>,
{
    /// Adds the sender to the queue for the given key.
    ///
    /// Returns true if this is the first queued sender for the key
    pub fn queue(&mut self, key: K, sender: S) -> bool {
        self.metrics.queued_consumers_count.increment(1.0);
        match self.queued.entry(key) {
            Entry::Occupied(mut entry) => {
                entry.get_mut().push(sender);
                false
            }
            Entry::Vacant(entry) => {
                entry.insert(vec![sender]);
                true
            }
        }
    }

    /// Remove consumers for a given key, this will also remove the key from the cache.
    pub fn remove(&mut self, key: &K) -> Option<Vec<S>>
    where
        V: InMemorySize,
    {
        self.cache
            .remove(key)
            .inspect(|value| self.memory_usage = self.memory_usage.saturating_sub(value.size()));
        self.queued
            .remove(key)
            .inspect(|removed| self.metrics.queued_consumers_count.decrement(removed.len() as f64))
    }

    /// Returns a reference to the value for a given key and promotes that element to be the most
    /// recently used.
    pub fn get(&mut self, key: &K) -> Option<&mut V> {
        let entry = self.cache.get(key);
        if entry.is_some() {
            self.metrics.hits_total.increment(1);
        } else {
            self.metrics.misses_total.increment(1);
        }
        entry
    }

    /// Inserts a new element into the map.
    ///
    /// Can fail if the element is rejected by the limiter or if we fail to grow an empty map.
    ///
    /// See [`Schnellru::insert`](LruMap::insert) for more info.
    pub fn insert<'a>(&mut self, key: L::KeyToInsert<'a>, value: V) -> bool
    where
        L::KeyToInsert<'a>: Hash + PartialEq<K>,
        V: InMemorySize,
    {
        let size = value.size();

        if self.cache.limiter().is_over_the_limit(self.cache.len() + 1) {
            if let Some((_, evicted)) = self.cache.pop_oldest() {
                // update tracked memory with the evicted value
                self.memory_usage = self.memory_usage.saturating_sub(evicted.size());
            }
        }

        if self.cache.insert(key, value) {
            self.memory_usage = self.memory_usage.saturating_add(size);
            true
        } else {
            false
        }
    }

    /// Shrinks the capacity of the queue with a lower limit.
    #[inline]
    pub fn shrink_to(&mut self, min_capacity: usize) {
        self.queued.shrink_to(min_capacity);
    }

    /// Update metrics for the inner cache.
    #[inline]
    pub fn update_cached_metrics(&self) {
        self.metrics.cached_count.set(self.cache.len() as f64);
        self.metrics.memory_usage.set(self.memory_usage as f64);
    }
}

impl<K, V, S> MultiConsumerLruCache<K, V, ByLength, S>
where
    K: Hash + Eq,
{
    /// Creates a new empty map with a given `max_len` and metric label.
    pub fn new(max_len: u32, cache_id: &str) -> Self {
        Self {
            cache: LruMap::new(ByLength::new(max_len)),
            queued: Default::default(),
            metrics: CacheMetrics::new_with_labels(&[("cache", cache_id.to_string())]),
            memory_usage: 0,
        }
    }
}

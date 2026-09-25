//! Metered cache, which also provides storage for senders in order to queue queries that result in
//! a cache miss.

use super::metrics::CacheMetrics;
use reth_primitives_traits::InMemorySize;
use schnellru::{ByLength, LruMap};
use std::{
    collections::{hash_map::Entry, HashMap},
    fmt::{self, Debug, Formatter},
    hash::Hash,
    time::Duration,
};
use tokio::time::Instant;

/// A multi-consumer LRU cache bounded by entry count and estimated payload size.
pub struct MultiConsumerLruCache<K: Hash + Eq, V, S> {
    /// The LRU cache.
    cache: LruMap<K, CachedValue<V>>,
    /// All queued consumers.
    queued: HashMap<K, Vec<S>>,
    /// Cache metrics.
    metrics: CacheMetrics,
    /// Tracked payload usage, excluding map and request queue overhead.
    memory_usage: usize,
    /// Maximum estimated payload bytes retained by the cache.
    max_bytes: usize,
    /// Whether the cached count or memory usage changed since the gauges were last published.
    metrics_dirty: bool,
}

impl<K: Hash + Eq, V, S> Debug for MultiConsumerLruCache<K, V, S> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("MultiConsumerLruCache")
            .field("cache_length", &self.cache.len())
            .field("cache_memory_usage", &self.cache.memory_usage())
            .field("queued_length", &self.queued.len())
            .field("memory_usage", &self.memory_usage)
            .finish()
    }
}

impl<K: Hash + Eq, V, S> MultiConsumerLruCache<K, V, S> {
    /// Creates an empty cache with entry and payload byte limits and a metric label.
    ///
    /// Setting either limit to zero disables caching, but consumers can still queue requests.
    pub fn new(max_len: u32, max_bytes: usize, cache_id: &str) -> Self {
        Self {
            cache: LruMap::new(ByLength::new(max_len)),
            queued: Default::default(),
            metrics: CacheMetrics::new_with_labels(&[("cache", cache_id.to_string())]),
            memory_usage: 0,
            max_bytes,
            metrics_dirty: false,
        }
    }

    /// Adds the sender to the queue for the given key.
    ///
    /// Returns true if this is the first queued sender for the key.
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

    /// Removes consumers for a given key, also removing the key from the cache.
    pub fn remove(&mut self, key: &K) -> Option<Vec<S>> {
        if let Some(entry) = self.cache.remove(key) {
            self.memory_usage -= entry.size;
            self.metrics_dirty = true;
        }
        self.queued
            .remove(key)
            .inspect(|removed| self.metrics.queued_consumers_count.decrement(removed.len() as f64))
    }

    /// Returns a reference to the value and refreshes its recency and idle timestamp.
    pub fn get(&mut self, key: &K) -> Option<&V> {
        self.get_at(key, Instant::now())
    }

    /// Returns a reference to the value and refreshes its recency using the supplied timestamp.
    ///
    /// Use nondecreasing timestamps for lookups and [`Self::insert_at`] to keep idle eviction in
    /// LRU order.
    pub fn get_at(&mut self, key: &K, now: Instant) -> Option<&V> {
        if let Some(entry) = self.cache.get(key) {
            entry.last_access = now;
            self.metrics.hits_total.increment(1);
            Some(&entry.value)
        } else {
            self.metrics.misses_total.increment(1);
            None
        }
    }

    /// Checks for a cached value without changing recency, idle timestamp, or lookup metrics.
    pub(crate) fn contains_key(&self, key: &K) -> bool {
        self.cache.peek(key).is_some()
    }

    /// Inserts a new element, evicting the oldest entries until both limits are satisfied.
    ///
    /// Oversized values are rejected without evicting existing entries or queued consumers.
    pub fn insert(&mut self, key: K, value: V) -> bool
    where
        V: InMemorySize,
    {
        self.insert_at(key, value, Instant::now())
    }

    /// Inserts an element using the supplied timestamp, enforcing the same limits as
    /// [`Self::insert`].
    ///
    /// Use the same nondecreasing timestamps as [`Self::get_at`] when batching cache operations.
    pub fn insert_at(&mut self, key: K, value: V, now: Instant) -> bool
    where
        V: InMemorySize,
    {
        if self.cache.limiter().max_length() == 0 || self.max_bytes == 0 {
            return false
        }
        let size = value.size();
        if size > self.max_bytes {
            return false
        }

        // Remove replacements before checking the limits so their size is not counted twice.
        if let Some(previous) = self.cache.remove(&key) {
            self.memory_usage -= previous.size;
            self.metrics_dirty = true;
        }
        while self.cache.len() >= self.cache.limiter().max_length() as usize ||
            self.memory_usage > self.max_bytes - size
        {
            self.pop_oldest();
        }

        let previous_len = self.cache.len();
        let inserted = self.cache.insert(key, CachedValue { value, size, last_access: now });
        if inserted {
            self.memory_usage += size;
            self.metrics_dirty = true;
        }
        if self.cache.len() != previous_len + usize::from(inserted) {
            // A failed allocation can make the LRU evict internally while trying to insert.
            self.memory_usage = self.cache.iter().map(|(_, entry)| entry.size).sum();
            self.metrics_dirty = true;
        }
        inserted
    }

    /// Shrinks the capacity of the queue with a lower limit.
    #[inline]
    pub fn shrink_to(&mut self, min_capacity: usize) {
        self.queued.shrink_to(min_capacity);
    }

    /// Publishes the cached count and memory usage gauges if either changed since the last call.
    ///
    /// Returns whether the gauges were written, so lookups that only hit the cache cost nothing.
    #[inline]
    pub fn update_cached_metrics(&mut self) -> bool {
        if !self.metrics_dirty {
            return false
        }
        self.metrics_dirty = false;
        self.metrics.cached_count.set(self.cache.len() as f64);
        self.metrics.memory_usage.set(self.memory_usage as f64);
        true
    }

    /// Removes at most five idle entries without disturbing consumers waiting for a database
    /// result.
    ///
    /// Returns whether more expired entries remain for the next poll.
    pub(crate) fn evict_expired(&mut self, now: Instant, idle_timeout: Duration) -> bool {
        if idle_timeout.is_zero() {
            return false
        }
        let mut remaining = 5;
        // Hits update timestamps and LRU order together, so the first live entry ends the scan.
        while self
            .cache
            .peek_oldest()
            .is_some_and(|(_, entry)| now.duration_since(entry.last_access) >= idle_timeout)
        {
            if remaining == 0 {
                return true
            }
            self.pop_oldest();
            remaining -= 1;
        }
        false
    }

    fn pop_oldest(&mut self) {
        if let Some((_, entry)) = self.cache.pop_oldest() {
            self.memory_usage -= entry.size;
            self.metrics_dirty = true;
        }
    }
}

/// Payload metadata measured once at insertion, so eviction never needs to traverse the payload.
struct CachedValue<V> {
    value: V,
    size: usize,
    last_access: Instant,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    #[derive(Debug, PartialEq, Eq)]
    struct Weighted(usize);

    impl InMemorySize for Weighted {
        fn size(&self) -> usize {
            self.0
        }
    }

    struct Measured(Arc<AtomicUsize>);

    impl InMemorySize for Measured {
        fn size(&self) -> usize {
            self.0.fetch_add(1, Ordering::Relaxed);
            3
        }
    }

    #[test]
    fn count_and_byte_limits_evict_oldest_entries() {
        let mut cache = MultiConsumerLruCache::<u64, _, ()>::new(3, 10, "test");
        for key in 0..3 {
            assert!(cache.insert(key, Weighted(2)));
        }
        assert!(cache.get(&0).is_some());
        assert!(cache.insert(3, Weighted(2)));
        assert!(cache.get(&1).is_none());
        assert_eq!(cache.memory_usage, 6);

        assert!(cache.insert(4, Weighted(8)));
        assert!(cache.get(&0).is_none());
        assert!(cache.get(&2).is_none());
        assert!(cache.get(&3).is_some());
        assert!(cache.get(&4).is_some());
        assert_eq!(cache.memory_usage, 10);
    }

    #[test]
    fn replacement_removal_and_oversized_values() {
        let mut cache = MultiConsumerLruCache::<u64, _, ()>::new(2, 10, "test");
        assert!(cache.insert(0, Weighted(6)));
        assert!(cache.insert(1, Weighted(4)));
        assert!(cache.insert(1, Weighted(2)));
        assert!(cache.get(&0).is_some());
        assert_eq!(cache.memory_usage, 8);

        assert!(cache.insert(1, Weighted(7)));
        assert!(cache.get(&0).is_none());
        assert_eq!(cache.memory_usage, 7);
        assert!(!cache.insert(1, Weighted(11)));
        assert!(!cache.insert(2, Weighted(11)));
        assert_eq!(cache.get(&1), Some(&Weighted(7)));
        assert_eq!(cache.memory_usage, 7);

        assert!(cache.insert(2, Weighted(10)));
        assert!(cache.get(&1).is_none());
        assert_eq!(cache.memory_usage, 10);
        cache.remove(&2);
        assert_eq!(cache.memory_usage, 0);
    }

    #[tokio::test(start_paused = true)]
    async fn idle_eviction_preserves_hits_and_queued_consumers() {
        let mut cache = MultiConsumerLruCache::new(3, 10, "test");
        let timeout = Duration::from_secs(10);
        for key in 0..3 {
            assert!(cache.insert(key, Weighted(3)));
        }
        assert!(cache.queue(1, 42));
        assert!(!cache.queue(1, 43));
        tokio::time::advance(timeout / 2).await;
        assert!(cache.get(&0).is_some());
        assert!(cache.contains_key(&1));
        tokio::time::advance(timeout / 2).await;
        cache.evict_expired(Instant::now(), timeout);
        assert_eq!(cache.memory_usage, 3);
        assert_eq!(cache.remove(&1), Some(vec![42, 43]));
        assert!(cache.contains_key(&0));
        tokio::time::advance(timeout / 2).await;
        cache.evict_expired(Instant::now(), Duration::ZERO);
        assert_eq!(cache.memory_usage, 3);
        cache.evict_expired(Instant::now(), timeout);
        assert_eq!(cache.memory_usage, 0);
    }

    #[test]
    fn payloads_are_only_measured_on_enabled_insertions() {
        for (count, bytes) in [(0, 10), (10, 0), (10, 3)] {
            let measurements = Arc::new(AtomicUsize::new(0));
            let mut cache = MultiConsumerLruCache::<u64, _, ()>::new(count, bytes, "test");
            let enabled = count > 0 && bytes > 0;
            assert!(cache.queue(0, ()));
            for key in [0, 1, 1] {
                assert_eq!(cache.insert(key, Measured(measurements.clone())), enabled);
                assert_eq!(cache.get(&key).is_some(), enabled);
            }
            cache.evict_expired(Instant::now() + Duration::from_secs(10), Duration::from_secs(10));
            assert_eq!(cache.memory_usage, 0);
            assert_eq!(measurements.load(Ordering::Relaxed), if enabled { 3 } else { 0 });
            assert_eq!(cache.remove(&0), Some(vec![()]));
        }
    }

    #[test]
    fn gauges_only_republished_after_cache_changes() {
        let mut cache = MultiConsumerLruCache::<u64, _, ()>::new(2, 10, "test");
        assert!(!cache.update_cached_metrics());
        assert!(cache.insert(0, Weighted(3)));
        assert!(cache.update_cached_metrics());
        assert!(!cache.update_cached_metrics());
        assert!(cache.get(&0).is_some());
        assert!(cache.get(&1).is_none());
        assert!(!cache.update_cached_metrics());
        cache.remove(&0);
        assert!(cache.update_cached_metrics());
    }

    #[tokio::test(start_paused = true)]
    async fn batched_timestamps_preserve_idle_eviction_order() {
        let mut cache = MultiConsumerLruCache::<u64, _, ()>::new(4, 12, "test");
        let start = Instant::now();
        let timeout = Duration::from_secs(10);
        for key in 0..3 {
            assert!(cache.insert_at(key, Weighted(3), start));
        }
        tokio::time::advance(timeout / 2).await;
        let now = Instant::now();
        assert!(cache.get_at(&0, now).is_some());

        // Insertions and later hits in a batch share its timestamp even if the clock advances.
        tokio::time::advance(timeout / 2).await;
        assert!(cache.insert_at(3, Weighted(3), now));
        assert!(cache.get_at(&2, now).is_some());
        cache.evict_expired(Instant::now(), timeout);
        assert!(!cache.contains_key(&1));
        assert_eq!(cache.memory_usage, 9);

        tokio::time::advance(timeout / 2).await;
        cache.evict_expired(Instant::now(), timeout);
        assert_eq!(cache.memory_usage, 0);
    }

    #[test]
    fn idle_eviction_is_bounded_and_preserves_renewed_entries() {
        let mut cache = MultiConsumerLruCache::new(12, 36, "test");
        let start = Instant::now();
        let timeout = Duration::from_secs(10);
        for key in 0..12 {
            assert!(cache.insert_at(key, Weighted(3), start));
        }
        assert!(cache.queue(0, 42));
        let now = start + timeout;

        assert!(cache.evict_expired(now, timeout));
        assert_eq!(cache.memory_usage, 21);
        assert!(!cache.contains_key(&4));
        assert!(cache.get_at(&5, now).is_some());

        assert!(cache.evict_expired(now, timeout));
        assert_eq!(cache.memory_usage, 6);
        assert!(!cache.evict_expired(now, timeout));
        assert_eq!(cache.memory_usage, 3);
        assert!(cache.contains_key(&5));
        assert_eq!(cache.remove(&0), Some(vec![42]));
    }
}

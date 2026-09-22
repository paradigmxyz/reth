//! Metered cache, which also provides storage for senders in order to queue queries that result in
//! a cache miss.

use super::metrics::CacheMetrics;
use reth_primitives_traits::InMemorySize;
use schnellru::{ByLength, Limiter, LruMap};
use std::{
    collections::{hash_map::Entry, HashMap},
    fmt::{self, Debug, Formatter},
    hash::Hash,
    time::Duration,
};
use tokio::time::Instant;

/// A multi-consumer LRU cache.
pub struct MultiConsumerLruCache<K, V, L, S>
where
    K: Hash + Eq,
    L: Limiter<K, V>,
{
    /// The LRU cache.
    cache: LruMap<K, V, MeteredLimiter<L, V>>,
    /// All queued consumers.
    queued: HashMap<K, Vec<S>>,
    /// Cache metrics
    metrics: CacheMetrics,
    /// Whether the cached count or memory usage changed since the gauges were last published.
    metrics_dirty: bool,
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
            .field("memory_usage", &self.cache.limiter().memory_usage)
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
        self.cache.remove(key).inspect(|_| {
            self.metrics_dirty = true;
        });
        self.queued
            .remove(key)
            .inspect(|removed| self.metrics.queued_consumers_count.decrement(removed.len() as f64))
    }

    /// Returns a reference to the value for a given key and promotes that element to be the most
    /// recently used.
    ///
    /// Mutating the value must not change its [`InMemorySize`] estimate.
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
    /// See [`LruMap::insert`] for more info.
    pub fn insert<'a>(&mut self, key: L::KeyToInsert<'a>, value: V) -> bool
    where
        L::KeyToInsert<'a>: Hash + PartialEq<K>,
        V: InMemorySize,
    {
        if self.cache.limiter().max_bytes.is_some_and(|limit| limit == 0 || value.size() > limit) {
            return false
        }
        self.cache.limiter_mut().value_size = V::size;
        let previous = (self.cache.len(), self.cache.limiter().memory_usage);
        let inserted = self.cache.insert(key, value);
        if !inserted {
            // Allocation or rehashing can fail after the limiter charges the incoming value,
            // without a removal callback for it. Reconcile the entries that actually survived.
            let retained =
                self.cache.iter().map(|(_, value)| value.size()).fold(0, usize::saturating_add);
            self.cache.limiter_mut().memory_usage = retained;
        }
        if previous != (self.cache.len(), self.cache.limiter().memory_usage) {
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
        self.metrics.memory_usage.set(self.cache.limiter().memory_usage as f64);
        true
    }

    #[cfg(test)]
    pub(super) fn memory_usage(&self) -> usize {
        self.cache.limiter().memory_usage
    }
}

impl<K, V, S> MultiConsumerLruCache<K, V, ByLength, S>
where
    K: Hash + Eq,
{
    /// Creates a new empty map with a given `max_len` and metric label.
    pub fn new(max_len: u32, cache_id: &str) -> Self {
        Self::new_with_limits(max_len, None, cache_id)
    }

    /// Creates a cache with both an entry limit and an optional payload byte limit.
    pub(super) fn new_with_limits(max_len: u32, max_bytes: Option<usize>, cache_id: &str) -> Self {
        Self {
            cache: LruMap::new(MeteredLimiter {
                inner: ByLength::new(max_len),
                memory_usage: 0,
                max_bytes,
                value_size: |_| 0,
            }),
            queued: Default::default(),
            metrics: CacheMetrics::new_with_labels(&[("cache", cache_id.to_string())]),
            metrics_dirty: false,
        }
    }
}

impl<K, P, L, S> MultiConsumerLruCache<K, CachedEntry<P>, L, S>
where
    K: Hash + Eq + Debug,
    L: Limiter<K, CachedEntry<P>>,
{
    /// Returns a live cached payload and renews its idle timeout on a hit.
    pub(super) fn get_cached(
        &mut self,
        key: &K,
        now: Option<Instant>,
        idle_timeout: Option<Duration>,
    ) -> Option<P>
    where
        P: Clone,
    {
        match self.cache.get(key) {
            Some(entry) if !entry.is_expired(now, idle_timeout) => {
                entry.last_access = now;
                self.metrics.hits_total.increment(1);
                return Some(entry.value.clone())
            }
            Some(_) => {
                self.cache.remove(key);
                self.metrics_dirty = true;
            }
            None => {}
        }
        self.metrics.misses_total.increment(1);
        None
    }

    /// Checks for a live payload without renewing its idle timeout or recording a lookup.
    pub(super) fn contains_cached(
        &self,
        key: &K,
        now: Option<Instant>,
        idle_timeout: Option<Duration>,
    ) -> bool {
        self.cache.peek(key).is_some_and(|entry| !entry.is_expired(now, idle_timeout))
    }

    /// Removes idle payloads without affecting consumers waiting for a database result.
    pub(super) fn evict_expired(&mut self, now: Instant, idle_timeout: Duration) {
        // Real hits update both the timestamp and LRU order, so live entries end the scan.
        while self
            .cache
            .peek_oldest()
            .is_some_and(|(_, entry)| entry.is_expired(Some(now), Some(idle_timeout)))
        {
            self.cache.pop_oldest();
            self.metrics_dirty = true;
        }
    }
}

impl<K, P, S> MultiConsumerLruCache<K, CachedEntry<P>, ByLength, S>
where
    K: Hash + Eq + Debug,
    P: InMemorySize,
{
    /// Measures a payload once before inserting it into the cache.
    pub(super) fn insert_cached(&mut self, key: K, value: P, now: Option<Instant>) -> bool {
        let limiter = self.cache.limiter();
        if limiter.inner.max_length() == 0 || limiter.max_bytes == Some(0) {
            return false
        }
        self.insert(key, CachedEntry::new(value, now))
    }
}

/// A shared RPC payload with its insertion-time size estimate and last real cache hit.
#[derive(Debug)]
pub(super) struct CachedEntry<P> {
    value: P,
    weight: usize,
    last_access: Option<Instant>,
}

impl<P: InMemorySize> CachedEntry<P> {
    pub(super) fn new(value: P, now: Option<Instant>) -> Self {
        Self { weight: value.size(), value, last_access: now }
    }
}

impl<P> CachedEntry<P> {
    fn is_expired(&self, now: Option<Instant>, idle_timeout: Option<Duration>) -> bool {
        match (self.last_access, now, idle_timeout) {
            (Some(last_access), Some(now), Some(timeout)) if !timeout.is_zero() => {
                now.duration_since(last_access) >= timeout
            }
            _ => false,
        }
    }
}

impl<P> InMemorySize for CachedEntry<P> {
    fn size(&self) -> usize {
        self.weight
    }
}

/// Tracks payload size for every removal, including evictions inside the LRU map.
struct MeteredLimiter<L, V> {
    inner: L,
    memory_usage: usize,
    max_bytes: Option<usize>,
    /// Initialized on insertion so empty caches need no `InMemorySize` bound.
    value_size: fn(&V) -> usize,
}

impl<K, V, L: Limiter<K, V>> Limiter<K, V> for MeteredLimiter<L, V> {
    type KeyToInsert<'a> = L::KeyToInsert<'a>;
    type LinkType = L::LinkType;

    fn is_over_the_limit(&self, length: usize) -> bool {
        self.inner.is_over_the_limit(length) ||
            self.max_bytes.is_some_and(|limit| self.memory_usage > limit)
    }

    fn on_insert(&mut self, length: usize, key: Self::KeyToInsert<'_>, value: V) -> Option<(K, V)> {
        let (key, value) = self.inner.on_insert(length, key, value)?;
        self.memory_usage = self.memory_usage.saturating_add((self.value_size)(&value));
        Some((key, value))
    }

    fn on_replace(
        &mut self,
        length: usize,
        old_key: &mut K,
        new_key: Self::KeyToInsert<'_>,
        old_value: &mut V,
        new_value: &mut V,
    ) -> bool {
        let old_size = (self.value_size)(old_value);
        let accepted = self.inner.on_replace(length, old_key, new_key, old_value, new_value);
        let retained_size = (self.value_size)(if accepted { new_value } else { old_value });
        self.memory_usage =
            self.memory_usage.saturating_sub(old_size).saturating_add(retained_size);
        accepted
    }

    fn on_removed(&mut self, key: &mut K, value: &mut V) {
        self.memory_usage = self.memory_usage.saturating_sub((self.value_size)(value));
        self.inner.on_removed(key, value);
    }

    fn on_cleared(&mut self) {
        self.memory_usage = 0;
        self.inner.on_cleared();
    }

    fn on_grow(&mut self, new_memory_usage: usize) -> bool {
        self.inner.on_grow(new_memory_usage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct Weighted(usize);

    impl InMemorySize for Weighted {
        fn size(&self) -> usize {
            self.0
        }
    }

    #[test]
    fn empty_cache_operations_do_not_require_in_memory_size() {
        struct Unmeasured;

        let mut cache: MultiConsumerLruCache<u64, Unmeasured, ByLength, ()> =
            MultiConsumerLruCache::new(2, "test");
        assert!(cache.get(&0).is_none());
        assert!(cache.queue(0, ()));
        assert!(!cache.queue(0, ()));
    }

    #[test]
    fn cached_lookup_hashes_once_before_optional_expired_removal() {
        #[derive(Debug)]
        struct CountedKey(u64, Arc<AtomicUsize>);

        impl PartialEq for CountedKey {
            fn eq(&self, other: &Self) -> bool {
                self.0 == other.0
            }
        }

        impl Eq for CountedKey {}

        impl Hash for CountedKey {
            fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
                self.1.fetch_add(1, Ordering::Relaxed);
                self.0.hash(state);
            }
        }

        let timeout = Duration::from_secs(10);
        for (idle_timeout, elapsed, key, hit, expected_hashes) in [
            (None, 0, 1, true, 1),
            (None, 0, 2, false, 1),
            (Some(timeout), 5, 1, true, 1),
            (Some(timeout), 5, 2, false, 1),
            (Some(timeout), 10, 1, false, 2),
        ] {
            let hashes = Arc::new(AtomicUsize::new(0));
            let mut cache: MultiConsumerLruCache<CountedKey, CachedEntry<u64>, ByLength, ()> =
                MultiConsumerLruCache::new(2, "test");
            let now = idle_timeout.map(|_| Instant::now());
            assert!(cache.insert_cached(CountedKey(1, hashes.clone()), 42, now));
            hashes.store(0, Ordering::Relaxed);

            let result = cache.get_cached(
                &CountedKey(key, hashes.clone()),
                now.map(|now| now + Duration::from_secs(elapsed)),
                idle_timeout,
            );
            assert_eq!(result.is_some(), hit);
            assert_eq!(hashes.load(Ordering::Relaxed), expected_hashes);
        }
    }

    #[test]
    fn cached_lookup_counts_expiry_as_a_miss_and_updates_gauges() {
        for max_bytes in [None, Some(10)] {
            let mut cache: MultiConsumerLruCache<u64, CachedEntry<Arc<Weighted>>, ByLength, u64> =
                MultiConsumerLruCache::new_with_limits(2, max_bytes, "test");
            let hits = Arc::new(metrics::atomics::AtomicU64::new(0));
            let misses = Arc::new(metrics::atomics::AtomicU64::new(0));
            cache.metrics.hits_total = metrics::Counter::from_arc(hits.clone());
            cache.metrics.misses_total = metrics::Counter::from_arc(misses.clone());

            let now = Instant::now();
            let timeout = Duration::from_secs(10);
            let value = Arc::new(Weighted(3));
            let retained = Arc::downgrade(&value);
            assert!(cache.insert_cached(1, value.clone(), Some(now)));
            assert!(cache.update_cached_metrics());
            let cached = cache.get_cached(&1, Some(now + timeout / 2), Some(timeout)).unwrap();
            assert!(Arc::ptr_eq(&cached, &value));
            drop(cached);
            drop(value);
            assert_eq!(hits.load(Ordering::Relaxed), 1);
            assert_eq!(misses.load(Ordering::Relaxed), 0);
            assert!(!cache.update_cached_metrics());

            assert!(cache.get_cached(&2, Some(now + timeout / 2), Some(timeout)).is_none());
            assert_eq!(misses.load(Ordering::Relaxed), 1);
            assert!(!cache.update_cached_metrics());

            assert!(cache.queue(1, 42));
            assert!(cache
                .get_cached(&1, Some(now + timeout + timeout / 2), Some(timeout))
                .is_none());
            assert_eq!(hits.load(Ordering::Relaxed), 1);
            assert_eq!(misses.load(Ordering::Relaxed), 2);
            assert_eq!(cache.memory_usage(), 0);
            assert!(cache.update_cached_metrics());
            assert!(!cache.update_cached_metrics());
            assert!(retained.upgrade().is_none());
            assert_eq!(cache.remove(&1), Some(vec![42]));
        }
    }

    #[test]
    fn only_real_hits_renew_idle_expiry() {
        let mut cache: MultiConsumerLruCache<u64, CachedEntry<Weighted>, ByLength, u64> =
            MultiConsumerLruCache::new(10, "test");
        let now = Instant::now();
        let timeout = Duration::from_secs(10);
        for key in 0..3 {
            assert!(cache.insert_cached(key, Weighted(3), Some(now)));
        }
        assert!(cache.queue(1, 42));
        assert!(cache.get_cached(&0, Some(now + timeout / 2), Some(timeout)).is_some());
        assert!(cache.contains_cached(&1, Some(now + timeout / 2), Some(timeout)));
        assert!(!cache.contains_cached(&1, Some(now + timeout), Some(timeout)));

        cache.evict_expired(now + timeout, timeout);
        assert_eq!(cache.memory_usage(), 3);
        assert!(cache.contains_cached(&0, Some(now + timeout), Some(timeout)));
        assert_eq!(cache.remove(&1), Some(vec![42]));

        cache.evict_expired(now + timeout + timeout / 2, timeout);
        assert_eq!(cache.memory_usage(), 0);
    }

    #[test]
    fn disabled_expiry_needs_no_timestamp() {
        let mut cache: MultiConsumerLruCache<u64, CachedEntry<Weighted>, ByLength, ()> =
            MultiConsumerLruCache::new(10, "test");
        assert!(cache.insert_cached(1, Weighted(3), None));
        assert!(cache.get_cached(&1, None, None).is_some());
        assert!(cache.contains_cached(&1, None, None));
        assert_eq!(cache.cache.peek(&1).unwrap().last_access, None);

        let now = Instant::now();
        assert!(cache.insert_cached(2, Weighted(3), Some(now)));
        cache.evict_expired(now + Duration::from_secs(60), Duration::ZERO);
        assert!(cache
            .get_cached(&2, Some(now + Duration::from_secs(60)), Some(Duration::ZERO))
            .is_some());
    }

    #[test]
    fn memory_budget_evicts_multiple_oldest_entries() {
        let mut cache: MultiConsumerLruCache<u64, Weighted, ByLength, ()> =
            MultiConsumerLruCache::new_with_limits(10, Some(10), "test");
        for key in 0..3 {
            assert!(cache.insert(key, Weighted(3)));
        }
        assert!(cache.get(&0).is_some());
        assert!(cache.insert(3, Weighted(6)));
        assert!(cache.get(&0).is_some());
        assert!(cache.get(&1).is_none());
        assert!(cache.get(&2).is_none());
        assert!(cache.get(&3).is_some());
        assert_eq!(cache.memory_usage(), 9);
    }

    #[test]
    fn count_and_memory_limits_both_apply() {
        let mut cache: MultiConsumerLruCache<u64, Weighted, ByLength, ()> =
            MultiConsumerLruCache::new_with_limits(2, Some(10), "test");
        assert!(cache.insert(0, Weighted(1)));
        assert!(cache.insert(1, Weighted(1)));
        assert!(cache.insert(2, Weighted(1)));
        assert!(cache.get(&0).is_none());
        assert_eq!(cache.memory_usage(), 2);

        // An entry exactly matching the byte budget can occupy the whole cache.
        assert!(cache.insert(3, Weighted(10)));
        assert!(cache.get(&1).is_none());
        assert!(cache.get(&2).is_none());
        assert_eq!(cache.memory_usage(), 10);
        assert!(cache.insert(3, Weighted(4)));
        assert_eq!(cache.memory_usage(), 4);
        assert!(cache.remove(&3).is_none());
        assert_eq!(cache.memory_usage(), 0);
    }

    #[test]
    fn disabled_or_oversized_insertions_preserve_the_cache() {
        for (count, bytes) in [(0, None), (0, Some(10)), (10, Some(0))] {
            let mut cache: MultiConsumerLruCache<u64, Weighted, ByLength, ()> =
                MultiConsumerLruCache::new_with_limits(count, bytes, "test");
            assert!(!cache.insert(0, Weighted(0)));
            assert!(!cache.insert(0, Weighted(1)));
            assert_eq!(cache.memory_usage(), 0);
            assert!(!cache.update_cached_metrics());
        }

        let mut cache: MultiConsumerLruCache<u64, Weighted, ByLength, ()> =
            MultiConsumerLruCache::new_with_limits(2, Some(10), "test");
        assert!(cache.insert(0, Weighted(4)));
        assert!(!cache.insert(1, Weighted(11)));
        assert!(!cache.insert(0, Weighted(11)));
        assert_eq!(cache.get(&0), Some(&mut Weighted(4)));
        assert_eq!(cache.memory_usage(), 4);
    }

    #[test]
    fn allocation_limit_keeps_accounting_consistent() {
        use schnellru::ByMemoryUsage;

        for limit in [0, 1024] {
            let mut cache: MultiConsumerLruCache<u64, Weighted, ByMemoryUsage, ()> =
                MultiConsumerLruCache {
                    cache: LruMap::new(MeteredLimiter {
                        inner: ByMemoryUsage::new(limit),
                        memory_usage: 0,
                        max_bytes: None,
                        value_size: |_| 0,
                    }),
                    queued: HashMap::new(),
                    metrics: CacheMetrics::new_with_labels(&[("cache", "test".to_owned())]),
                    metrics_dirty: false,
                };
            for key in 0..100 {
                assert_eq!(cache.insert(key, Weighted(3)), limit != 0);
                assert_eq!(cache.memory_usage(), cache.cache.len() * 3);
            }
            assert!(cache.cache.len() < 100);
        }
    }

    #[test]
    fn payload_size_is_only_measured_on_insertion() {
        #[derive(Clone)]
        struct Measured(Arc<AtomicUsize>);

        impl InMemorySize for Measured {
            fn size(&self) -> usize {
                self.0.fetch_add(1, Ordering::Relaxed);
                3
            }
        }

        let measurements = Arc::new(AtomicUsize::new(0));
        let mut cache: MultiConsumerLruCache<u64, CachedEntry<Measured>, ByLength, ()> =
            MultiConsumerLruCache::new_with_limits(10, Some(3), "test");
        let now = Instant::now();
        let timeout = Duration::from_secs(10);
        assert!(cache.insert_cached(0, Measured(measurements.clone()), Some(now)));
        assert!(cache.get_cached(&0, Some(now), Some(timeout)).is_some());
        assert!(cache.insert_cached(1, Measured(measurements.clone()), Some(now)));
        assert!(cache.insert_cached(1, Measured(measurements.clone()), Some(now)));
        cache.evict_expired(now + timeout, timeout);
        assert_eq!(cache.memory_usage(), 0);
        assert_eq!(measurements.load(Ordering::Relaxed), 3);
    }

    #[test]
    fn disabled_cache_does_not_measure_payloads() {
        struct Measured(Arc<AtomicUsize>);

        impl InMemorySize for Measured {
            fn size(&self) -> usize {
                self.0.fetch_add(1, Ordering::Relaxed);
                3
            }
        }

        for (count, bytes, enabled) in
            [(0, None, false), (10, Some(0), false), (10, None, true), (10, Some(3), true)]
        {
            let measurements = Arc::new(AtomicUsize::new(0));
            let mut cache: MultiConsumerLruCache<u64, CachedEntry<Measured>, ByLength, u64> =
                MultiConsumerLruCache::new_with_limits(count, bytes, "test");
            assert!(cache.queue(1, 42));

            assert_eq!(cache.insert_cached(1, Measured(measurements.clone()), None), enabled);
            assert_eq!(measurements.load(Ordering::Relaxed), usize::from(enabled));
            assert_eq!(cache.memory_usage(), if enabled { 3 } else { 0 });
            assert_eq!(cache.update_cached_metrics(), enabled);
            assert_eq!(cache.remove(&1), Some(vec![42]));
        }
    }

    #[test]
    fn replacement_does_not_double_count_memory() {
        let mut cache: MultiConsumerLruCache<u64, u64, ByLength, ()> =
            MultiConsumerLruCache::new(2, "test");
        assert!(cache.insert(1, 10));
        assert!(cache.insert(1, 20));
        assert_eq!(cache.memory_usage(), size_of::<u64>());
        assert_eq!(cache.get(&1), Some(&mut 20));
    }

    #[test]
    fn gauges_only_republished_after_cache_changes() {
        let mut cache: MultiConsumerLruCache<u64, u64, ByLength, ()> =
            MultiConsumerLruCache::new(2, "test");
        assert!(!cache.update_cached_metrics());

        assert!(cache.insert(0, 0));
        assert!(cache.update_cached_metrics());
        assert!(!cache.update_cached_metrics());

        // hits do not touch the gauges
        assert!(cache.get(&0).is_some());
        assert!(cache.get(&1).is_none());
        assert!(!cache.update_cached_metrics());

        assert!(cache.remove(&0).is_none());
        assert!(cache.update_cached_metrics());
    }
}

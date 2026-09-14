//! Network cache support

use alloy_primitives::map::DefaultHashBuilder;
use core::hash::BuildHasher;
use derive_more::{Deref, DerefMut};
use itertools::Itertools;
use schnellru::{ByLength, Limiter, Unlimited};
use std::{fmt, hash::Hash};

/// A minimal LRU cache based on a [`LruMap`](schnellru::LruMap) with limited capacity.
///
/// If the length exceeds the set capacity, the oldest element will be removed
/// In the limit, for each element inserted the oldest existing element will be removed.
pub struct LruCache<T: Hash + Eq + fmt::Debug, S = DefaultHashBuilder>
where
    S: BuildHasher,
{
    limit: u32,
    inner: LruMap<T, (), ByLength, S>,
}

impl<T: Hash + Eq + fmt::Debug> LruCache<T> {
    /// Creates a new [`LruCache`] using the given limit
    pub fn new(limit: u32) -> Self {
        Self::with_hasher(limit, Default::default())
    }
}

impl<T: Hash + Eq + fmt::Debug, S: BuildHasher> LruCache<T, S> {
    /// Creates a new [`LruCache`] using the given limit and hash builder.
    pub const fn with_hasher(limit: u32, hash_builder: S) -> Self {
        // limit of lru map is one element more, so can give eviction feedback, which isn't
        // supported by LruMap
        Self { inner: LruMap::with_hasher(limit + 1, hash_builder), limit }
    }

    /// Insert an element into the set.
    ///
    /// This operation uses `get_or_insert` from the underlying `schnellru::LruMap` which:
    /// - Automatically evicts the least recently used item if capacity is exceeded
    ///
    /// This method is more efficient than [`insert_and_get_evicted`](Self::insert_and_get_evicted)
    /// as it performs fewer checks. Use this method when you don't need information about
    /// evicted values.
    ///
    /// If the set did not have this value present, true is returned.
    /// If the set did have this value present, false is returned.
    pub fn insert(&mut self, entry: T) -> bool {
        let mut is_new = false;
        self.inner.get_or_insert(entry, || {
            is_new = true;
        });
        is_new
    }

    /// Inserts a duplicate-free sequence and returns the number of cache hits.
    ///
    /// After a capacity-sized prefix of distinct entries, every later entry must be a miss.
    /// The final capacity-sized suffix determines the cache's contents and recency order.
    pub(crate) fn insert_unique(&mut self, entries: &[T]) -> usize
    where
        T: Clone,
    {
        // Ordinary insert uses the backing map's limit, including its extra eviction slot.
        let capacity = self.inner.limiter().max_length() as usize;
        if capacity == 0 {
            return 0
        }
        let suffix_start = capacity.max(entries.len().saturating_sub(capacity));
        entries
            .iter()
            .take(capacity)
            .chain(entries.iter().skip(suffix_start))
            .filter(|entry| !self.insert((*entry).clone()))
            .count()
    }

    /// Same as [`insert`](Self::insert) but returns a tuple, where the second index is the evicted
    /// value, if one was evicted.
    pub fn insert_and_get_evicted(&mut self, entry: T) -> (bool, Option<T>) {
        let new = self.inner.peek(&entry).is_none();
        let evicted =
            (new && (self.limit as usize) <= self.inner.len()).then(|| self.remove_lru()).flatten();
        _ = self.inner.get_or_insert(entry, || ());

        (new, evicted)
    }

    /// Gets the given element, if exists, and promotes to lru.
    pub fn get(&mut self, entry: &T) -> Option<&T> {
        let _ = self.inner.get(entry)?;
        self.iter().next()
    }

    /// Iterates through entries and returns a reference to the given entry, if exists, without
    /// promoting to lru.
    ///
    /// NOTE: Use this for type that have custom impl of [`PartialEq`] and [`Eq`], that aren't
    /// unique by all fields. If `PartialEq` and `Eq` are derived for a type, it's more efficient to
    /// call [`contains`](Self::contains).
    pub fn find(&self, entry: &T) -> Option<&T> {
        self.iter().find(|key| *key == entry)
    }

    /// Remove the least recently used entry and return it.
    ///
    /// If the `LruCache` is empty or if the eviction feedback is
    /// configured, this will return None.
    #[inline]
    fn remove_lru(&mut self) -> Option<T> {
        self.inner.pop_oldest().map(|(k, ())| k)
    }

    /// Expels the given value. Returns true if the value existed.
    pub fn remove(&mut self, value: &T) -> bool {
        self.inner.remove(value).is_some()
    }

    /// Returns `true` if the set contains a value.
    pub fn contains(&self, value: &T) -> bool {
        self.inner.peek(value).is_some()
    }

    /// Returns an iterator over all cached entries in lru order
    pub fn iter(&self) -> impl Iterator<Item = &T> + '_ {
        self.inner.iter().map(|(k, ())| k)
    }

    /// Returns number of elements currently in cache.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns `true` if there are currently no elements in the cache.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

impl<T, S> Extend<T> for LruCache<T, S>
where
    T: Eq + Hash + fmt::Debug,
    S: BuildHasher,
{
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        for item in iter {
            _ = self.insert(item);
        }
    }
}

impl<T, S> fmt::Debug for LruCache<T, S>
where
    T: fmt::Debug + Hash + Eq,
    S: BuildHasher,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug_struct = f.debug_struct("LruCache");

        debug_struct.field("limit", &self.limit);

        debug_struct.field(
            "ret %iter",
            &format_args!("Iter: {{{} }}", self.iter().map(|k| format!(" {k:?}")).format(",")),
        );

        debug_struct.finish()
    }
}

/// Wrapper of [`schnellru::LruMap`] that implements [`fmt::Debug`] and with the common hash
/// builder.
#[derive(Deref, DerefMut, Default)]
pub struct LruMap<K, V, L = ByLength, S = DefaultHashBuilder>(schnellru::LruMap<K, V, L, S>)
where
    K: Hash + PartialEq,
    L: Limiter<K, V>,
    S: BuildHasher;

impl<K, V, L, S> fmt::Debug for LruMap<K, V, L, S>
where
    K: Hash + PartialEq + fmt::Display,
    V: fmt::Debug,
    L: Limiter<K, V> + fmt::Debug,
    S: BuildHasher,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug_struct = f.debug_struct("LruMap");

        debug_struct.field("limiter", self.limiter());

        debug_struct.field(
            "ret %iter",
            &format_args!(
                "Iter: {{{} }}",
                self.iter().map(|(k, v)| format!(" {k}: {v:?}")).format(",")
            ),
        );

        debug_struct.finish()
    }
}

impl<K, V> LruMap<K, V>
where
    K: Hash + PartialEq,
{
    /// Returns a new cache with default limiter and hash builder.
    pub fn new(max_length: u32) -> Self {
        Self::with_hasher(max_length, Default::default())
    }
}

impl<K, V, S> LruMap<K, V, ByLength, S>
where
    K: Hash + PartialEq,
    S: BuildHasher,
{
    /// Returns a new cache with default limiter and the given hash builder.
    pub const fn with_hasher(max_length: u32, hash_builder: S) -> Self {
        Self(schnellru::LruMap::with_hasher(ByLength::new(max_length), hash_builder))
    }
}

impl<K, V> LruMap<K, V, Unlimited>
where
    K: Hash + PartialEq,
{
    /// Returns a new cache with [`Unlimited`] limiter and default hash builder.
    pub fn new_unlimited() -> Self {
        Self(schnellru::LruMap::with_hasher(Unlimited, Default::default()))
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use derive_more::{Constructor, Display};
    use std::hash::Hasher;

    #[test]
    fn insert_unique_matches_sequential_hits_and_recency() {
        for limit in [0, 1, 2, 3, 319, 320] {
            let capacity = limit as usize + 1;
            for len in [
                0,
                1,
                capacity - 1,
                capacity,
                capacity + 1,
                2 * capacity - 1,
                2 * capacity,
                2 * capacity + 1,
                4096,
            ] {
                for offset in [0, 1, capacity, capacity + 1] {
                    let mut sequential = LruCache::new(limit);
                    let mut batched = LruCache::new(limit);
                    sequential.extend(0..capacity);
                    batched.extend(0..capacity);
                    let entries: Vec<_> =
                        (0..len).map(|index| (index + offset) % (len + capacity)).collect();
                    // Repeat the announcement to exercise eviction between messages as well.
                    for _ in 0..2 {
                        let expected_hits =
                            entries.iter().filter(|entry| !sequential.insert(**entry)).count();
                        assert_eq!(batched.insert_unique(&entries), expected_hits);
                        assert_eq!(
                            batched.iter().collect::<Vec<_>>(),
                            sequential.iter().collect::<Vec<_>>()
                        );
                    }
                }
            }
        }
    }

    #[derive(Debug, Hash, PartialEq, Eq, Display, Clone, Copy)]
    struct Key(i8);

    #[derive(Debug, Eq, Constructor, Clone, Copy)]
    struct CompoundKey {
        // type unique for id
        id: i8,
        other: i8,
    }

    impl PartialEq for CompoundKey {
        fn eq(&self, other: &Self) -> bool {
            self.id == other.id
        }
    }

    impl Hash for CompoundKey {
        fn hash<H: Hasher>(&self, state: &mut H) {
            self.id.hash(state)
        }
    }

    #[test]
    fn test_cache_should_insert_into_empty_set() {
        let mut cache = LruCache::new(5);
        let entry = "entry";
        assert!(cache.insert(entry));
        assert!(cache.contains(&entry));
    }

    #[test]
    fn test_cache_should_not_insert_same_value_twice() {
        let mut cache = LruCache::new(5);
        let entry = "entry";
        assert!(cache.insert(entry));
        assert!(!cache.insert(entry));
    }

    #[test]
    fn test_cache_should_remove_oldest_element_when_exceeding_limit() {
        let mut cache = LruCache::new(1); // LruCache limit will be 2, check LruCache::new
        let old_entry = "old_entry";
        let new_entry = "new_entry";
        cache.insert(old_entry);
        cache.insert("entry");
        cache.insert(new_entry);
        assert!(cache.contains(&new_entry));
        assert!(!cache.contains(&old_entry));
    }

    #[test]
    fn test_cache_should_extend_an_array() {
        let mut cache = LruCache::new(5);
        let entries = ["some_entry", "another_entry"];
        cache.extend(entries);
        for e in entries {
            assert!(cache.contains(&e));
        }
    }

    #[test]
    #[expect(dead_code)]
    fn test_debug_impl_lru_map() {
        #[derive(Debug)]
        struct Value(i8);

        let mut cache = LruMap::new(2);
        let key_1 = Key(1);
        let value_1 = Value(11);
        cache.insert(key_1, value_1);
        let key_2 = Key(2);
        let value_2 = Value(22);
        cache.insert(key_2, value_2);

        assert_eq!(
            "LruMap { limiter: ByLength { max_length: 2 }, ret %iter: Iter: { 2: Value(22), 1: Value(11) } }",
            format!("{cache:?}")
        )
    }

    #[test]
    fn test_debug_impl_lru_cache() {
        let mut cache = LruCache::new(2);
        let key_1 = Key(1);
        cache.insert(key_1);
        let key_2 = Key(2);
        cache.insert(key_2);

        assert_eq!(
            "LruCache { limit: 2, ret %iter: Iter: { Key(2), Key(1) } }",
            format!("{cache:?}")
        )
    }

    #[test]
    fn get() {
        let mut cache = LruCache::new(2);
        let key_1 = Key(1);
        cache.insert(key_1);
        let key_2 = Key(2);
        cache.insert(key_2);

        // promotes key 1 to lru
        _ = cache.get(&key_1);

        assert_eq!(
            "LruCache { limit: 2, ret %iter: Iter: { Key(1), Key(2) } }",
            format!("{cache:?}")
        )
    }

    #[test]
    fn get_ty_custom_eq_impl() {
        let mut cache = LruCache::new(2);
        let key_1 = CompoundKey::new(1, 11);
        cache.insert(key_1);
        let key_2 = CompoundKey::new(2, 22);
        cache.insert(key_2);

        let key = cache.get(&key_1);

        assert_eq!(key_1.other, key.unwrap().other)
    }

    #[test]
    fn peek() {
        let mut cache = LruCache::new(2);
        let key_1 = Key(1);
        cache.insert(key_1);
        let key_2 = Key(2);
        cache.insert(key_2);

        // doesn't promote key 1 to lru
        _ = cache.find(&key_1);

        assert_eq!(
            "LruCache { limit: 2, ret %iter: Iter: { Key(2), Key(1) } }",
            format!("{cache:?}")
        )
    }

    #[test]
    fn peek_ty_custom_eq_impl() {
        let mut cache = LruCache::new(2);
        let key_1 = CompoundKey::new(1, 11);
        cache.insert(key_1);
        let key_2 = CompoundKey::new(2, 22);
        cache.insert(key_2);

        let key = cache.find(&key_1);

        assert_eq!(key_1.other, key.unwrap().other)
    }

    #[test]
    fn test_insert_methods() {
        let mut cache = LruCache::new(2);

        // Test basic insert
        assert!(cache.insert("first")); // new entry
        assert!(!cache.insert("first")); // existing entry
        assert!(cache.insert("second")); // new entry

        // Test insert_and_get_evicted
        let (is_new, evicted) = cache.insert_and_get_evicted("third");
        assert!(is_new); // should be new entry
        assert_eq!(evicted, Some("first")); // should evict

        assert!(cache.contains(&"second"));
        assert!(cache.contains(&"third"));
        assert!(!cache.contains(&"first"));

        // Test insert_and_get_evicted with existing entry
        let (is_new, evicted) = cache.insert_and_get_evicted("second");
        assert!(!is_new); // should not be new
        assert_eq!(evicted, None); // should not evict anything
    }
}

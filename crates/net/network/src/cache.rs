use core::hash::BuildHasher;
use derive_more::{Deref, DerefMut};
use itertools::Itertools;
use linked_hash_set::LinkedHashSet;
use schnellru::{ByLength, Limiter, RandomState, Unlimited};
use std::{borrow::Borrow, fmt, hash::Hash, num::NonZeroUsize};

/// A minimal LRU cache based on a `LinkedHashSet` with limited capacity.
///
/// If the length exceeds the set capacity, the oldest element will be removed
/// In the limit, for each element inserted the oldest existing element will be removed.
#[derive(Clone)]
pub struct LruCache<T: Hash + Eq> {
    limit: NonZeroUsize,
    inner: LinkedHashSet<T>,
}

impl<T: Hash + Eq> LruCache<T> {
    /// Creates a new [`LruCache`] using the given limit
    pub fn new(limit: NonZeroUsize) -> Self {
        Self { inner: LinkedHashSet::new(), limit }
    }

    /// Insert an element into the set.
    ///
    /// If the element is new (did not exist before [`LruCache::insert()`]) was called, then the
    /// given length will be enforced and the oldest element will be removed if the limit was
    /// exceeded.
    ///
    /// If the set did not have this value present, true is returned.
    /// If the set did have this value present, false is returned.
    pub fn insert(&mut self, entry: T) -> bool {
        let (new_entry, _evicted_val) = self.insert_and_get_evicted(entry);
        new_entry
    }

    /// Same as [`Self::insert`] but returns a tuple, where the second index is the evicted value,
    /// if one was evicted.
    pub fn insert_and_get_evicted(&mut self, entry: T) -> (bool, Option<T>) {
        if self.inner.insert(entry) {
            if self.limit.get() < self.inner.len() {
                // remove the oldest element in the set
                return (true, self.remove_lru())
            }
            return (true, None)
        }
        (false, None)
    }

    /// Remove the least recently used entry and return it.
    ///
    /// If the `LruCache` is empty or if the eviction feedback is
    /// configured, this will return None.
    #[inline]
    fn remove_lru(&mut self) -> Option<T> {
        self.inner.pop_front()
    }

    /// Expels the given value. Returns true if the value existed.
    pub fn remove(&mut self, value: &T) -> bool {
        self.inner.remove(value)
    }

    /// Returns `true` if the set contains a value.
    pub fn contains<Q: ?Sized>(&self, value: &Q) -> bool
    where
        T: Borrow<Q>,
        Q: Hash + Eq,
    {
        self.inner.contains(value)
    }

    /// Returns an iterator over all cached entries in lru order
    pub fn iter(&self) -> impl Iterator<Item = &T> + '_ {
        self.inner.iter().rev()
    }

    /// Returns number of elements currently in cache.
    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns `true` if there are currently no elements in the cache.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

impl<T> Extend<T> for LruCache<T>
where
    T: Eq + Hash,
{
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        for item in iter.into_iter() {
            self.insert(item);
        }
    }
}

impl<T> fmt::Debug for LruCache<T>
where
    T: fmt::Debug + Hash + Eq,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug_struct = f.debug_struct("LruCache");

        debug_struct.field("limit", &self.limit);

        debug_struct.field(
            "res_fn_iter",
            &format_args!("Iter: {{{} }}", self.iter().map(|k| format!(" {k:?}")).format(",")),
        );

        debug_struct.finish()
    }
}

/// Wrapper of [`schnellru::LruMap`] that implements [`fmt::Debug`].
#[derive(Deref, DerefMut, Default)]
pub struct LruMap<K, V, L = ByLength, S = RandomState>(schnellru::LruMap<K, V, L, S>)
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
            "res_fn_iter",
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
        LruMap(schnellru::LruMap::new(ByLength::new(max_length)))
    }
}

impl<K, V> LruMap<K, V, Unlimited>
where
    K: Hash + PartialEq,
{
    /// Returns a new cache with [`Unlimited`] limiter and default hash builder.
    pub fn new_unlimited() -> Self {
        LruMap(schnellru::LruMap::new(Unlimited))
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_cache_should_insert_into_empty_set() {
        let limit = NonZeroUsize::new(5).unwrap();
        let mut cache = LruCache::new(limit);
        let entry = "entry";
        assert!(cache.insert(entry));
        assert!(cache.contains(entry));
    }

    #[test]
    fn test_cache_should_not_insert_same_value_twice() {
        let limit = NonZeroUsize::new(5).unwrap();
        let mut cache = LruCache::new(limit);
        let entry = "entry";
        assert!(cache.insert(entry));
        assert!(!cache.insert(entry));
    }

    #[test]
    fn test_cache_should_remove_oldest_element_when_exceeding_limit() {
        let limit = NonZeroUsize::new(2).unwrap();
        let mut cache = LruCache::new(limit);
        let old_entry = "old_entry";
        let new_entry = "new_entry";
        cache.insert(old_entry);
        cache.insert("entry");
        cache.insert(new_entry);
        assert!(cache.contains(new_entry));
        assert!(!cache.contains(old_entry));
    }

    #[test]
    fn test_cache_should_extend_an_array() {
        let limit = NonZeroUsize::new(5).unwrap();
        let mut cache = LruCache::new(limit);
        let entries = ["some_entry", "another_entry"];
        cache.extend(entries);
        for e in entries {
            assert!(cache.contains(e));
        }
    }

    #[test]
    #[allow(dead_code)]
    fn test_debug_impl_lru_map() {
        use derive_more::Display;

        #[derive(Debug, Hash, PartialEq, Eq, Display)]
        struct Key(i8);

        #[derive(Debug)]
        struct Value(i8);

        let mut cache = LruMap::new(2);
        let key_1 = Key(1);
        let value_1 = Value(11);
        cache.insert(key_1, value_1);
        let key_2 = Key(2);
        let value_2 = Value(22);
        cache.insert(key_2, value_2);

        assert_eq!("LruMap { limiter: ByLength { max_length: 2 }, res_fn_iter: Iter: { 2: Value(22), 1: Value(11) } }", format!("{cache:?}"))
    }

    #[test]
    #[allow(dead_code)]
    fn test_debug_impl_lru_cache() {
        #[derive(Debug, Hash, PartialEq, Eq)]
        struct Key(i8);

        let mut cache = LruCache::new(NonZeroUsize::new(2).unwrap());
        let key_1 = Key(1);
        cache.insert(key_1);
        let key_2 = Key(2);
        cache.insert(key_2);

        assert_eq!(
            "LruCache { limit: 2, res_fn_iter: Iter: { Key(2), Key(1) } }",
            format!("{cache:?}")
        )
    }
}

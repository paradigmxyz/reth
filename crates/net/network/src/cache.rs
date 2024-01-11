use derive_more::{Deref, DerefMut};
use linked_hash_set::LinkedHashSet;
use schnellru::{self, ByLength};
use std::{
    borrow::Borrow,
    fmt::{self, Write},
    hash,
    hash::Hash,
    num::NonZeroUsize,
};

/// A minimal LRU cache based on a `LinkedHashSet` with limited capacity.
///
/// If the length exceeds the set capacity, the oldest element will be removed
/// In the limit, for each element inserted the oldest existing element will be removed.
#[derive(Debug, Clone)]
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
        if self.inner.insert(entry) {
            if self.limit.get() == self.inner.len() {
                // remove the oldest element in the set
                _ = self.remove_lru();
            }
            return true
        }
        false
    }

    /// Same as [`Self::insert`] but returns a tuple, where the second index is the evicted value,
    /// if one was evicted.
    pub fn _insert_with_eviction_feedback(&mut self, entry: T) -> (bool, Option<T>) {
        if self.inner.insert(entry) {
            if self.limit.get() == self.inner.len() {
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
    pub fn _remove(&mut self, value: &T) -> bool {
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

    /// Returns an iterator over all cached entries
    pub fn iter(&self) -> impl Iterator<Item = &T> + '_ {
        self.inner.iter()
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

/// Wrapper of [`schnellru::LruMap`] that implements [`fmt::Debug`].
#[derive(Deref, DerefMut)]
pub struct LruMap<K, V>(schnellru::LruMap<K, V>)
where
    K: hash::Hash + PartialEq;

impl<K, V> fmt::Debug for LruMap<K, V>
where
    K: hash::Hash + PartialEq + fmt::Display,
    V: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug_struct = f.debug_struct("LruMap");
        for (k, v) in self.0.iter() {
            let mut key_str = String::new();
            write!(&mut key_str, "{k}")?;
            debug_struct.field(&key_str, &v);
        }
        debug_struct.finish()
    }
}

impl<K, V> LruMap<K, V>
where
    K: hash::Hash + PartialEq,
{
    pub fn new(max_length: u32) -> Self {
        Self(schnellru::LruMap::new(ByLength::new(max_length)))
    }
}

impl<K, V> From<LruMap<K, V>> for schnellru::LruMap<K, V>
where
    K: hash::Hash + PartialEq,
{
    fn from(value: LruMap<K, V>) -> Self {
        value.0
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

        assert_eq!("LruMap { 2: Value(22), 1: Value(11) }", format!("{cache:?}"))
    }
}

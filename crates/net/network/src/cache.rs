use linked_hash_set::LinkedHashSet;
use std::{borrow::Borrow, hash::Hash, num::NonZeroUsize};

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
    /// Creates a new `LruCache` using the given limit
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
                self.remove_lru();
            }
            return true
        }
        false
    }

    /// Remove the least recently used entry and return it.
    ///
    /// If the `LruCache` is empty this will return None.
    #[inline]
    fn remove_lru(&mut self) {
        self.inner.pop_front();
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
}

use linked_hash_set::LinkedHashSet;
use std::{hash::Hash, num::NonZeroUsize};

/// A minimal LRU cache based on a `LinkedHashSet` with limited capacity.
///
/// If the length exceeds the set capacity, the oldest element will be removed
/// In the limit, for each element inserted the oldest existing element will be removed.
#[derive(Debug, Clone)]
pub struct LruCache<T: Hash + Eq> {
    limit: NonZeroUsize,
    set: LinkedHashSet<T>,
}

impl<T: Hash + Eq> LruCache<T> {
    /// Creates a new `LruCache` using the given limit
    pub fn new(limit: NonZeroUsize) -> Self {
        Self { set: LinkedHashSet::new(), limit }
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
        if self.set.insert(entry) {
            if self.limit.get() == self.set.len() {
                // remove the oldest element in the set
                self.set.pop_front();
            }
            return true
        }
        false
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

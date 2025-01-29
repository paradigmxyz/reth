use super::HashedCursor;
use alloy_primitives::{map::B256HashMap, B256};
use reth_storage_errors::db::DatabaseError;

/// Cache for hashed cursors.
#[derive(Clone, Debug)]
pub struct HashedCursorCache<V> {
    seek: B256HashMap<Option<(B256, V)>>,
    next: B256HashMap<Option<(B256, V)>>,
}

impl<V> Default for HashedCursorCache<V> {
    fn default() -> Self {
        Self { seek: B256HashMap::default(), next: B256HashMap::default() }
    }
}

impl<V> HashedCursorCache<V> {
    /// Extend cache with contents of the other.
    pub fn extend(&mut self, other: Self) {
        self.seek.extend(other.seek);
        self.next.extend(other.next);
    }
}

/// Hashed cursor that cached `seek` and `next` results.
#[derive(Debug)]
pub struct CachedHashedCursor<C, V> {
    cursor: C,
    cache: HashedCursorCache<V>,
    last: Option<B256>,
}

impl<C, V> CachedHashedCursor<C, V> {
    /// Create new cached cursor.
    pub fn new(cursor: C, cache: HashedCursorCache<V>) -> Self {
        Self { cursor, cache, last: None }
    }

    /// Take cache.
    pub fn take_cache(&mut self) -> HashedCursorCache<V> {
        std::mem::take(&mut self.cache)
    }
}

impl<C, V: Clone + std::fmt::Debug> HashedCursor for CachedHashedCursor<C, V>
where
    C: HashedCursor<Value = V>,
{
    type Value = C::Value;

    fn seek(&mut self, key: B256) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        self.last = Some(key);
        if let Some(entry) = self.cache.seek.get(&key).cloned() {
            return Ok(entry)
        }
        let entry = self.cursor.seek(key)?;
        self.cache.seek.insert(key, entry.clone());
        Ok(entry)
    }

    fn next(&mut self) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        let next = match self.last {
            Some(key) => {
                if let Some(entry) = self.cache.next.get(&key).cloned() {
                    return Ok(entry)
                }
                // First position the cursor at last entry.
                self.cursor.seek(key)?;
                let entry = self.cursor.next()?;
                self.cache.next.insert(key, entry.clone());
                entry
            }
            // no previous entry was found
            None => None,
        };
        Ok(next)
    }
}

use super::HashedCursor;
use alloy_primitives::B256;
use moka::sync::Cache;
use reth_storage_errors::db::DatabaseError;

/// Cache for hashed cursors.
#[derive(Clone, Debug)]
pub struct HashedCursorCache<V: Clone + Send + Sync + 'static> {
    /// Seek cache.
    pub seek: Cache<B256, Option<(B256, V)>>,
    /// Next cache.
    pub next: Cache<B256, Option<(B256, V)>>,
}

impl<V> Default for HashedCursorCache<V>
where
    V: Clone + Send + Sync + 'static,
{
    fn default() -> Self {
        Self { seek: Cache::new(10_000), next: Cache::new(10_000) }
    }
}

/// Hashed cursor that cached `seek` and `next` results.
#[derive(Debug)]
pub struct CachedHashedCursor<C, V: Clone + Send + Sync + 'static> {
    cursor: C,
    cache: HashedCursorCache<V>,
    last: Option<B256>,
}

impl<C, V> CachedHashedCursor<C, V>
where
    V: Clone + Send + Sync + 'static,
{
    /// Create new cached cursor.
    pub fn new(cursor: C, cache: HashedCursorCache<V>) -> Self {
        Self { cursor, cache, last: None }
    }
}

impl<C, V> HashedCursor for CachedHashedCursor<C, V>
where
    C: HashedCursor<Value = V>,
    V: Clone + std::fmt::Debug + Send + Sync + 'static,
{
    type Value = C::Value;

    fn seek(&mut self, key: B256) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        let entry = if let Some(entry) = self.cache.seek.get(&key) {
            entry
        } else {
            let entry = self.cursor.seek(key)?;
            self.cache.seek.insert(key, entry.clone());
            entry
        };
        self.last = entry.as_ref().map(|e| e.0);
        Ok(entry)
    }

    fn next(&mut self) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        let next = match self.last {
            Some(key) => {
                let entry = if let Some(entry) = self.cache.next.get(&key) {
                    entry
                } else {
                    // First position the cursor at last entry.
                    self.cursor.seek(key)?;
                    let entry = self.cursor.next()?;
                    self.cache.next.insert(key, entry.clone());
                    entry
                };
                self.last = entry.as_ref().map(|e| e.0);
                entry
            }
            // no previous entry was found
            None => None,
        };
        Ok(next)
    }
}

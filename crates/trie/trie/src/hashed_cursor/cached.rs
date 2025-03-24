use std::{collections::hash_map::Entry, fmt::Debug};

use alloy_primitives::{map::B256Map, B256, U256};
use reth_primitives_traits::Account;
use reth_storage_errors::db::DatabaseError;

use super::{HashedCursor, HashedCursorFactory, HashedStorageCursor};

/// The hashed cursor factory that creates cursors that cache the visited keys.
#[derive(Clone, Debug)]
pub struct CachedHashedCursorFactory<CF> {
    cursor_factory: CF,
}

impl<CF> CachedHashedCursorFactory<CF> {
    /// Create a new factory.
    pub const fn new(cursor_factory: CF) -> Self {
        Self { cursor_factory }
    }

    #[cfg(test)]
    pub(crate) const fn inner(&self) -> &CF {
        &self.cursor_factory
    }
}

impl<CF: HashedCursorFactory> HashedCursorFactory for CachedHashedCursorFactory<CF> {
    type AccountCursor = CachedHashedCursor<CF::AccountCursor, Account>;
    type StorageCursor = CachedHashedCursor<CF::StorageCursor, U256>;

    fn hashed_account_cursor(&self) -> Result<Self::AccountCursor, DatabaseError> {
        Ok(CachedHashedCursor::new(self.cursor_factory.hashed_account_cursor()?))
    }

    fn hashed_storage_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageCursor, DatabaseError> {
        Ok(CachedHashedCursor::new(self.cursor_factory.hashed_storage_cursor(hashed_address)?))
    }
}

/// The hashed cursor that caches the visited keys.
#[derive(Debug, Default)]
pub struct CachedHashedCursor<C, T> {
    cursor: C,
    /// The current key.
    ///
    /// Set to `B256::ZERO` if the cursor has just been created, is empty, has been seeked to a
    /// non- existent key, or has been advanced beyond the last key.
    current_key: B256,
    /// The cache of [`Self::seek`] calls.
    ///
    /// The key is the seeked key, and the value is the result of the seek.
    ///
    /// This map is also populated:
    /// - During the [`Self::seek`] calls using the key that the cursor actually seeked to.
    /// - During the [`Self::next`] calls using the key that the cursor actually advanced to.
    cached_seeks: B256Map<Option<(B256, T)>>,
    /// The cache of [`Self::next`] calls.
    ///
    /// The key is the previous key before calling [`Self::next`], and the value is the result of
    /// the call.
    cached_nexts: B256Map<Option<(B256, T)>>,
}

impl<C, T> CachedHashedCursor<C, T>
where
    C: HashedCursor<Value = T>,
{
    fn new(cursor: C) -> Self {
        Self {
            cursor,
            current_key: B256::ZERO,
            cached_seeks: Default::default(),
            cached_nexts: Default::default(),
        }
    }
}

impl<C, T: Debug + Copy> HashedCursor for CachedHashedCursor<C, T>
where
    C: HashedCursor<Value = T>,
{
    type Value = T;

    /// Seeks to the given key.
    ///
    /// If the key is already cached, the value will be returned from the cache.
    /// Otherwise, the underlying cursor will be seeked to the given key.
    ///
    /// The result of the seek will be cached, and the key that the underlying cursor seeked to
    /// will be cached as well if it differs from the seeked key.
    fn seek(&mut self, key: B256) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        let (result, actual_seek) = match self.cached_seeks.entry(key) {
            Entry::Occupied(entry) => (*entry.get(), None),
            Entry::Vacant(entry) => {
                let result = self.cursor.seek(key)?;
                self.current_key = result.as_ref().map(|(k, _)| *k).unwrap_or(B256::ZERO);
                entry.insert(result);

                let actual_seek = result.filter(|(k, _)| k != &key).map(|(k, _)| k);

                (result, actual_seek)
            }
        };

        if let Some(actual_seek) = actual_seek {
            self.cached_seeks.insert(actual_seek, result);
        }

        Ok(result)
    }

    /// Advances to the next key.
    ///
    /// If the next value after the current key is already cached, it will be returned from the
    /// cache. Otherwise, the underlying cursor will be advanced to the next key.
    ///
    /// The result of the advance will be cached in both the cache of [`Self::seek`] calls and the
    /// cache of [`Self::next`] calls.
    fn next(&mut self) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        Ok(match self.cached_nexts.entry(self.current_key) {
            Entry::Occupied(entry) => *entry.get(),
            Entry::Vacant(entry) => {
                let result = self.cursor.next()?;
                self.current_key = result.as_ref().map(|(k, _)| *k).unwrap_or(B256::ZERO);
                if let Some((key, value)) = result.as_ref() {
                    self.cached_seeks.insert(*key, Some((*key, *value)));
                }
                entry.insert(result);
                result
            }
        })
    }
}

impl<C, T: Debug + Copy> HashedStorageCursor for CachedHashedCursor<C, T>
where
    C: HashedStorageCursor<Value = T>,
{
    fn is_storage_empty(&mut self) -> Result<bool, DatabaseError> {
        self.cursor.is_storage_empty()
    }
}

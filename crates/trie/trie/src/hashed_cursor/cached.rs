use std::{collections::hash_map::Entry, fmt::Debug};

use alloy_primitives::{map::B256Map, B256, U256};
use reth_primitives_traits::Account;
use reth_storage_errors::db::DatabaseError;

use super::{HashedCursor, HashedCursorFactory, HashedStorageCursor};

#[derive(Clone, Debug)]
pub struct CachedHashedCursorFactory<CF> {
    cursor_factory: CF,
}

impl<CF> CachedHashedCursorFactory<CF> {
    /// Create a new factory.
    pub const fn new(cursor_factory: CF) -> Self {
        Self { cursor_factory }
    }

    pub const fn inner(&self) -> &CF {
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

#[derive(Debug, Default)]
pub struct CachedHashedCursor<C, T> {
    cursor: C,
    current_key: B256,
    cached_seeks: B256Map<Option<(B256, T)>>,
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

    fn seek(&mut self, key: B256) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        Ok(match self.cached_seeks.entry(key) {
            Entry::Occupied(entry) => *entry.get(),
            Entry::Vacant(entry) => {
                let result = self.cursor.seek(key)?;
                self.current_key = result.as_ref().map(|(k, _)| *k).unwrap_or(B256::ZERO);
                entry.insert(result);
                result
            }
        })
    }

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

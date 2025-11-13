use super::{HashedCursor, HashedCursorFactory, HashedStorageCursor};
use alloy_primitives::{B256, U256};
use core::marker::PhantomData;
use reth_primitives_traits::Account;
use reth_storage_errors::db::DatabaseError;

/// Noop hashed cursor factory.
#[derive(Clone, Default, Debug)]
#[non_exhaustive]
pub struct NoopHashedCursorFactory;

impl HashedCursorFactory for NoopHashedCursorFactory {
    type AccountCursor<'a>
        = NoopHashedCursor<Account>
    where
        Self: 'a;
    type StorageCursor<'a>
        = NoopHashedCursor<U256>
    where
        Self: 'a;

    fn hashed_account_cursor(&self) -> Result<Self::AccountCursor<'_>, DatabaseError> {
        Ok(NoopHashedCursor::default())
    }

    fn hashed_storage_cursor(
        &self,
        _hashed_address: B256,
    ) -> Result<Self::StorageCursor<'_>, DatabaseError> {
        Ok(NoopHashedCursor::default())
    }
}

/// Generic noop hashed cursor.
#[derive(Debug)]
pub struct NoopHashedCursor<V> {
    _marker: PhantomData<V>,
}

impl<V> Default for NoopHashedCursor<V> {
    fn default() -> Self {
        Self { _marker: PhantomData }
    }
}

impl<V> HashedCursor for NoopHashedCursor<V>
where
    V: std::fmt::Debug,
{
    type Value = V;

    fn seek(&mut self, _key: B256) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        Ok(None)
    }

    fn next(&mut self) -> Result<Option<(B256, Self::Value)>, DatabaseError> {
        Ok(None)
    }

    fn reset(&mut self) {
        // Noop
    }
}

impl HashedStorageCursor for NoopHashedCursor<U256> {
    fn is_storage_empty(&mut self) -> Result<bool, DatabaseError> {
        Ok(true)
    }

    fn set_hashed_address(&mut self, _hashed_address: B256) {
        // Noop
    }
}

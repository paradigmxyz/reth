use alloy_primitives::{B256, U256};
use reth_primitives_traits::Account;
use reth_storage_errors::db::DatabaseError;

/// Implementation of hashed state cursor traits for the post state.
mod post_state;
pub use post_state::*;

/// Implementation of noop hashed state cursor.
pub mod noop;

/// Mock trie cursor implementations.
#[cfg(any(test, feature = "test-utils"))]
pub mod mock;

/// Metrics tracking hashed cursor implementations.
pub mod metrics;
#[cfg(feature = "metrics")]
pub use metrics::HashedCursorMetrics;
pub use metrics::{HashedCursorMetricsCache, InstrumentedHashedCursor};

/// The factory trait for creating cursors over the hashed state.
#[auto_impl::auto_impl(&)]
pub trait HashedCursorFactory {
    /// The hashed account cursor type.
    type AccountCursor<'a>: HashedCursor<Value = Account>
    where
        Self: 'a;
    /// The hashed storage cursor type.
    type StorageCursor<'a>: HashedStorageCursor<Value = U256>
    where
        Self: 'a;

    /// Returns a cursor for iterating over all hashed accounts in the state.
    fn hashed_account_cursor(&self) -> Result<Self::AccountCursor<'_>, DatabaseError>;

    /// Returns a cursor for iterating over all hashed storage entries in the state.
    fn hashed_storage_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageCursor<'_>, DatabaseError>;

    /// Returns a cursor over hashed accounts that have at least one storage entry.
    fn hashed_storage_key_cursor(
        &self,
    ) -> Result<Option<Box<dyn HashedStorageKeyCursor + '_>>, DatabaseError> {
        Ok(None)
    }
}

/// The cursor for iterating over hashed entries.
#[auto_impl::auto_impl(&mut)]
pub trait HashedCursor {
    /// Value returned by the cursor.
    type Value: std::fmt::Debug;

    /// Seek an entry greater than or equal to the given key and position the cursor there.
    /// Returns the first entry with the key greater than or equal to the sought key.
    fn seek(&mut self, key: B256) -> Result<Option<(B256, Self::Value)>, DatabaseError>;

    /// Move the cursor to the next entry and return it.
    fn next(&mut self) -> Result<Option<(B256, Self::Value)>, DatabaseError>;

    /// Reset the cursor to its initial state.
    ///
    /// # Important
    ///
    /// After calling this method, the subsequent operation MUST be a [`HashedCursor::seek`] call.
    fn reset(&mut self);
}

/// The cursor for iterating over hashed storage entries.
#[auto_impl::auto_impl(&mut)]
pub trait HashedStorageCursor: HashedCursor {
    /// Returns `true` if there are no entries for a given key.
    fn is_storage_empty(&mut self) -> Result<bool, DatabaseError>;

    /// Set the hashed address for the storage cursor.
    ///
    /// # Important
    ///
    /// After calling this method, the subsequent operation MUST be a [`HashedCursor::seek`] call.
    fn set_hashed_address(&mut self, hashed_address: B256);
}

/// The cursor for iterating over hashed account keys that have storage entries.
pub trait HashedStorageKeyCursor {
    /// Seek the first storage account key greater than or equal to `hashed_address`.
    fn seek_storage_key(&mut self, hashed_address: B256) -> Result<Option<B256>, DatabaseError>;

    /// Move to the next storage account key, skipping duplicate storage slots.
    fn next_storage_key(&mut self) -> Result<Option<B256>, DatabaseError>;

    /// Returns the number of storage entries for the current storage account key when supported.
    fn current_storage_entry_count(&mut self) -> Result<Option<usize>, DatabaseError> {
        Ok(None)
    }
}

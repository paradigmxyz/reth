use reth_primitives::{Account, B256, U256};
use std::fmt::Debug;

/// Implementation of hashed state cursor traits for the post state.
mod post_state;
pub use post_state::*;

/// The factory trait for creating cursors over the hashed state.
pub trait HashedCursorFactory {
    /// An associated error that might occur on the cursor operation.
    type Err: Debug;
    /// The hashed account cursor type.
    type AccountCursor: HashedCursor<Value = Account>;
    /// The hashed storage cursor type.
    type StorageCursor: HashedStorageCursor<Value = U256>;

    /// Returns a cursor for iterating over all hashed accounts in the state.
    fn hashed_account_cursor(&self) -> Result<Self::AccountCursor, Self::Err>;

    /// Returns a cursor for iterating over all hashed storage entries in the state.
    fn hashed_storage_cursor(&self, hashed_address: B256)
        -> Result<Self::StorageCursor, Self::Err>;
}

/// The cursor for iterating over hashed entries.
pub trait HashedCursor {
    /// An associated error that might occur on the cursor operation.
    type Err: Debug;
    /// Value returned by the cursor.
    type Value: std::fmt::Debug;

    /// Seek an entry greater or equal to the given key and position the cursor there.
    /// Returns the first entry with the key greater or equal to the sought key.
    fn seek(&mut self, key: B256) -> Result<Option<(B256, Self::Value)>, Self::Err>;

    /// Move the cursor to the next entry and return it.
    fn next(&mut self) -> Result<Option<(B256, Self::Value)>, Self::Err>;
}

/// The cursor for iterating over hashed storage entries.
pub trait HashedStorageCursor: HashedCursor {
    /// Returns `true` if there are no entries for a given key.
    fn is_storage_empty(&mut self) -> Result<bool, Self::Err>;
}

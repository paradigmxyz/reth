use reth_primitives::{Account, StorageEntry, H256};

/// Default implementation of the hashed state cursor traits.
mod default;

/// Implementation of hashed state cursor traits for the post state.
mod post_state;
pub use post_state::*;

/// The factory trait for creating cursors over the hashed state.
pub trait HashedCursorFactory<'a> {
    /// The hashed account cursor type.
    type AccountCursor: HashedAccountCursor
    where
        Self: 'a;
    /// The hashed storage cursor type.
    type StorageCursor: HashedStorageCursor
    where
        Self: 'a;

    /// Returns a cursor for iterating over all hashed accounts in the state.
    fn hashed_account_cursor(&'a self) -> Result<Self::AccountCursor, reth_db::Error>;

    /// Returns a cursor for iterating over all hashed storage entries in the state.
    fn hashed_storage_cursor(&'a self) -> Result<Self::StorageCursor, reth_db::Error>;
}

/// The cursor for iterating over hashed accounts.
pub trait HashedAccountCursor {
    /// Seek an entry greater or equal to the given key and position the cursor there.
    fn seek(&mut self, key: H256) -> Result<Option<(H256, Account)>, reth_db::Error>;

    /// Move the cursor to the next entry and return it.
    fn next(&mut self) -> Result<Option<(H256, Account)>, reth_db::Error>;
}

/// The cursor for iterating over hashed storage entries.
pub trait HashedStorageCursor {
    /// Returns `true` if there are no entries for a given key.
    fn is_empty(&mut self, key: H256) -> Result<bool, reth_db::Error>;

    /// Seek an entry greater or equal to the given key/subkey and position the cursor there.
    fn seek(&mut self, key: H256, subkey: H256) -> Result<Option<StorageEntry>, reth_db::Error>;

    /// Move the cursor to the next entry and return it.
    fn next(&mut self) -> Result<Option<StorageEntry>, reth_db::Error>;
}

//! Owned database transaction for trie verification.

use crate::{
    DatabaseAccountTrieCursor, DatabaseHashedAccountCursor, DatabaseHashedStorageCursor,
    DatabaseStorageTrieCursor, TrieTableAdapter,
};
use alloy_primitives::B256;
use reth_db_api::{tables, transaction::DbTx, DatabaseError};
use reth_trie::{hashed_cursor::HashedCursorFactory, trie_cursor::TrieCursorFactory};
use std::marker::PhantomData;

/// Cursor factory owning one long-lived read transaction for a verifier thread.
///
/// Open all verifier transactions while commits are prevented, before applying any repairs.
#[derive(Debug)]
pub struct VerifierCursorFactory<TX, A> {
    tx: TX,
    adapter: PhantomData<A>,
}

impl<TX: DbTx, A: TrieTableAdapter> VerifierCursorFactory<TX, A> {
    /// Takes ownership of a read transaction and disables its long-read timeout. Move this
    /// factory into one storage worker or the lookahead thread; transactions are never shared.
    pub fn new(mut tx: TX) -> Self {
        tx.disable_long_read_transaction_safety();
        Self { tx, adapter: PhantomData }
    }
}

impl<TX: DbTx, A: TrieTableAdapter> HashedCursorFactory for VerifierCursorFactory<TX, A> {
    type AccountCursor<'a>
        = DatabaseHashedAccountCursor<TX::Cursor<tables::HashedAccounts>>
    where
        Self: 'a;
    type StorageCursor<'a>
        = DatabaseHashedStorageCursor<TX::DupCursor<tables::HashedStorages>>
    where
        Self: 'a;

    fn hashed_account_cursor(&self) -> Result<Self::AccountCursor<'_>, DatabaseError> {
        Ok(DatabaseHashedAccountCursor::new(self.tx.cursor_read::<tables::HashedAccounts>()?))
    }

    fn hashed_storage_cursor(
        &self,
        address: B256,
    ) -> Result<Self::StorageCursor<'_>, DatabaseError> {
        Ok(DatabaseHashedStorageCursor::new(
            self.tx.cursor_dup_read::<tables::HashedStorages>()?,
            address,
        ))
    }
}

impl<TX: DbTx, A: TrieTableAdapter> TrieCursorFactory for VerifierCursorFactory<TX, A> {
    type AccountTrieCursor<'a>
        = DatabaseAccountTrieCursor<TX::Cursor<A::AccountTrieTable>, A>
    where
        Self: 'a;
    type StorageTrieCursor<'a>
        = DatabaseStorageTrieCursor<TX::DupCursor<A::StorageTrieTable>, A>
    where
        Self: 'a;

    fn account_trie_cursor(&self) -> Result<Self::AccountTrieCursor<'_>, DatabaseError> {
        Ok(DatabaseAccountTrieCursor::new(self.tx.cursor_read::<A::AccountTrieTable>()?))
    }

    fn storage_trie_cursor(
        &self,
        address: B256,
    ) -> Result<Self::StorageTrieCursor<'_>, DatabaseError> {
        Ok(DatabaseStorageTrieCursor::new(
            self.tx.cursor_dup_read::<A::StorageTrieTable>()?,
            address,
        ))
    }
}

//! Database snapshots for parallel trie verification.

use crate::{
    DatabaseAccountTrieCursor, DatabaseHashedAccountCursor, DatabaseHashedStorageCursor,
    DatabaseStorageTrieCursor, TrieTableAdapter,
};
use alloy_primitives::B256;
use reth_db_api::{tables, transaction::DbTx, DatabaseError};
use reth_trie::{
    hashed_cursor::HashedCursorFactory, trie_cursor::TrieCursorFactory,
    verify::default_storage_workers,
};
use std::marker::PhantomData;

/// Cursor factories for [`reth_trie::verify::Verifier`] with independent, long-lived transactions
/// for the account thread, lookahead thread, and each storage worker.
///
/// All transactions are opened before verification starts, so repairs cannot change their view.
/// Storage cursors select the transaction belonging to the verifier pool's current worker.
#[derive(Debug)]
pub struct VerifierCursorFactory<TX, A> {
    account: TX,
    lookahead: TX,
    storage: Vec<TX>,
    adapter: PhantomData<A>,
}

impl<TX: DbTx, A: TrieTableAdapter> VerifierCursorFactory<TX, A> {
    /// Opens one long-lived read transaction per thread, with timeouts disabled. The caller must
    /// prevent database commits while these snapshots are opened and keep the node offline while
    /// verification runs. `open` is called once for each transaction; its first error is returned.
    pub fn new<E>(mut open: impl FnMut() -> Result<TX, E>) -> Result<Self, E> {
        let mut open = || {
            let mut tx = open()?;
            tx.disable_long_read_transaction_safety();
            Ok(tx)
        };
        let account = open()?;
        let lookahead = open()?;
        let storage =
            (0..default_storage_workers()).map(|_| open()).collect::<Result<Vec<_>, _>>()?;
        Ok(Self { account, lookahead, storage, adapter: PhantomData })
    }

    fn storage_tx(&self) -> &TX {
        if let Some(index) = rayon::current_thread_index() {
            &self.storage[index]
        } else {
            &self.lookahead
        }
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
        Ok(DatabaseHashedAccountCursor::new(
            self.lookahead.cursor_read::<tables::HashedAccounts>()?,
        ))
    }

    fn hashed_storage_cursor(
        &self,
        address: B256,
    ) -> Result<Self::StorageCursor<'_>, DatabaseError> {
        Ok(DatabaseHashedStorageCursor::new(
            self.storage_tx().cursor_dup_read::<tables::HashedStorages>()?,
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
        Ok(DatabaseAccountTrieCursor::new(self.account.cursor_read::<A::AccountTrieTable>()?))
    }

    fn storage_trie_cursor(
        &self,
        address: B256,
    ) -> Result<Self::StorageTrieCursor<'_>, DatabaseError> {
        Ok(DatabaseStorageTrieCursor::new(
            self.storage_tx().cursor_dup_read::<A::StorageTrieTable>()?,
            address,
        ))
    }
}

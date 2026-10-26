//! Database-backed trie verification.

use crate::{
    DatabaseAccountTrieCursor, DatabaseHashedAccountCursor, DatabaseHashedCursorFactory,
    DatabaseTrieCursorFactory, TrieTableAdapter,
};
use alloy_primitives::B256;
use reth_db_api::{
    cursor::{DbCursorRO, DbDupCursorRO},
    database::Database,
    tables,
    transaction::{CursorTy, DbTx, DupCursorTy},
    DatabaseError,
};
use reth_execution_errors::StateRootError;
use reth_trie::{
    hashed_cursor::HashedCursorFactory,
    trie_cursor::TrieCursorFactory,
    verify::{Output, StorageJob, StorageWorker, Verifier},
};
use std::fmt;

/// [`Verifier`] reading database tables through cursors of a `TX` transaction.
type TxVerifier<TX, A> = Verifier<
    DatabaseAccountTrieCursor<CursorTy<TX, <A as TrieTableAdapter>::AccountTrieTable>, A>,
    DatabaseHashedAccountCursor<CursorTy<TX, tables::HashedAccounts>>,
>;

/// Checks the trie tables against the hashed state tables of a database, yielding an [`Output`]
/// for every inconsistency found. See [`Verifier`] for details.
pub struct DatabaseVerifier<TX: DbTx, A: TrieTableAdapter> {
    verifier: TxVerifier<TX, A>,
    // Backs the cursors held by `verifier`, so it's declared, and dropped, after it.
    _txs: [TX; 2],
}

impl<TX, A> DatabaseVerifier<TX, A>
where
    TX: DbTx + 'static,
    A: TrieTableAdapter,
{
    /// Starts verification over read transactions opened through `db`.
    ///
    /// Prevent commits while this constructor opens the transactions so they read the same database
    /// version. Each storage worker owns a separate transaction. For long-running verification,
    /// configure `db` to disable long-lived read transaction safety. `storage_root_limit` bounds
    /// storage jobs whose requested roots have not yet been consumed, including storage without a
    /// hashed account row.
    ///
    /// # Panics
    ///
    /// Panics if `storage_workers` or `storage_root_limit` is zero.
    pub fn new(
        db: &impl Database<TX = TX>,
        storage_workers: usize,
        storage_root_limit: usize,
    ) -> Result<Self, DatabaseError> {
        assert!(storage_workers > 0, "at least one storage worker is required");
        let account_tx = db.tx()?;
        let lookahead_tx = db.tx()?;
        let storage_workers = (0..storage_workers)
            .map(|_| {
                let tx = db.tx()?;
                Ok(StorageWorker {
                    trie: DatabaseTrieCursorFactory::<_, A>::new(&tx)
                        .storage_trie_cursor(B256::ZERO)?,
                    hashed: DatabaseHashedCursorFactory::new(&tx)
                        .hashed_storage_cursor(B256::ZERO)?,
                    tx,
                })
            })
            .collect::<Result<Vec<_>, DatabaseError>>()?;
        let verifier = Verifier::new(
            DatabaseTrieCursorFactory::<_, A>::new(&account_tx).account_trie_cursor()?,
            DatabaseHashedCursorFactory::new(&account_tx).hashed_account_cursor()?,
            StorageAddresses::<TX, A> {
                hashed: lookahead_tx.cursor_dup_read::<tables::HashedStorages>()?,
                trie: lookahead_tx.cursor_dup_read::<A::StorageTrieTable>()?,
                next_hashed: None,
                next_trie: None,
                initialized: false,
            },
            storage_workers,
            storage_root_limit,
        )?;
        Ok(Self { verifier, _txs: [account_tx, lookahead_tx] })
    }
}

impl<TX: DbTx, A: TrieTableAdapter> Iterator for DatabaseVerifier<TX, A> {
    type Item = Result<Output, StateRootError>;

    fn next(&mut self) -> Option<Self::Item> {
        self.verifier.next()
    }
}

impl<TX: DbTx, A: TrieTableAdapter> fmt::Debug for DatabaseVerifier<TX, A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DatabaseVerifier").finish_non_exhaustive()
    }
}

/// Merges the distinct addresses in hashed storage and stored storage tries, including orphan
/// tries.
struct StorageAddresses<TX: DbTx, A: TrieTableAdapter> {
    hashed: DupCursorTy<TX, tables::HashedStorages>,
    trie: DupCursorTy<TX, A::StorageTrieTable>,
    next_hashed: Option<B256>,
    next_trie: Option<B256>,
    initialized: bool,
}

impl<TX: DbTx, A: TrieTableAdapter> StorageAddresses<TX, A> {
    fn next_address(&mut self) -> Result<Option<StorageJob>, DatabaseError> {
        if !self.initialized {
            self.next_hashed = self.hashed.first()?.map(|(address, _)| address);
            self.next_trie = self.trie.first()?.map(|(address, _)| address);
            self.initialized = true;
        }
        let address = match (self.next_hashed, self.next_trie) {
            (Some(hashed), Some(trie)) => hashed.min(trie),
            (Some(address), None) | (None, Some(address)) => address,
            (None, None) => return Ok(None),
        };
        // Non-empty hashed storage needs an account leaf even without a hashed account row.
        let has_storage = self.next_hashed == Some(address);
        if has_storage {
            self.next_hashed = self.hashed.next_no_dup()?.map(|(address, _)| address);
        }
        if self.next_trie == Some(address) {
            self.next_trie = self.trie.next_no_dup()?.map(|(address, _)| address);
        }
        Ok(Some(StorageJob { address, root_required: has_storage }))
    }
}

impl<TX: DbTx, A: TrieTableAdapter> Iterator for StorageAddresses<TX, A> {
    type Item = Result<StorageJob, DatabaseError>;

    fn next(&mut self) -> Option<Self::Item> {
        self.next_address().transpose()
    }
}

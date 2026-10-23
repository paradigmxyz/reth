//! Database-backed trie verification.

use crate::{
    DatabaseAccountTrieCursor, DatabaseHashedAccountCursor, DatabaseHashedCursorFactory,
    DatabaseHashedStorageCursor, DatabaseTrieCursorFactory, TrieTableAdapter,
};
use alloy_primitives::B256;
use reth_db_api::{
    tables,
    transaction::{CursorTy, DbTx, DupCursorTy},
    DatabaseError,
};
use reth_execution_errors::StateRootError;
use reth_trie::{
    hashed_cursor::HashedCursorFactory,
    trie_cursor::TrieCursorFactory,
    verify::{Output, StorageWorker, Verifier},
};
use std::fmt;

/// [`Verifier`] reading database tables through cursors of a `TX` transaction.
type TxVerifier<TX, A> = Verifier<
    DatabaseAccountTrieCursor<CursorTy<TX, <A as TrieTableAdapter>::AccountTrieTable>, A>,
    DatabaseHashedAccountCursor<CursorTy<TX, tables::HashedAccounts>>,
    DatabaseHashedStorageCursor<DupCursorTy<TX, tables::HashedStorages>>,
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
    /// Starts verification over read transactions returned by `open_tx`, which is called once for
    /// every transaction the verifier needs.
    ///
    /// Every transaction returned by `open_tx` must read the same database version. Long-lived
    /// read transaction safety is disabled on each of them. Each storage worker owns a separate
    /// transaction.
    ///
    /// # Panics
    ///
    /// Panics if `storage_workers` is zero.
    pub fn new<E: From<DatabaseError>>(
        mut open_tx: impl FnMut() -> Result<TX, E>,
        storage_workers: usize,
    ) -> Result<Self, E> {
        assert!(storage_workers > 0, "at least one storage worker is required");
        let mut open_tx = || {
            let mut tx = open_tx()?;
            tx.disable_long_read_transaction_safety();
            Ok::<_, E>(tx)
        };
        let account_tx = open_tx()?;
        let lookahead_tx = open_tx()?;
        let storage_workers = (0..storage_workers)
            .map(|_| {
                let tx = open_tx()?;
                Ok(StorageWorker {
                    trie: DatabaseTrieCursorFactory::<_, A>::new(&tx)
                        .storage_trie_cursor(B256::ZERO)?,
                    hashed: DatabaseHashedCursorFactory::new(&tx)
                        .hashed_storage_cursor(B256::ZERO)?,
                    tx,
                })
            })
            .collect::<Result<Vec<_>, E>>()?;
        let verifier = Verifier::new(
            DatabaseTrieCursorFactory::<_, A>::new(&account_tx).account_trie_cursor()?,
            DatabaseHashedCursorFactory::new(&account_tx).hashed_account_cursor()?,
            DatabaseHashedCursorFactory::new(&account_tx).hashed_storage_cursor(B256::ZERO)?,
            DatabaseHashedCursorFactory::new(&lookahead_tx).hashed_account_cursor()?,
            storage_workers,
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

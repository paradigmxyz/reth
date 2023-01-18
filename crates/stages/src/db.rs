#![allow(dead_code)]
use std::{
    fmt::Debug,
    ops::{Deref, DerefMut},
};

use reth_db::{
    cursor::{DbCursorRO, DbCursorRW},
    database::{Database, DatabaseGAT},
    models::{BlockNumHash, StoredBlockBody},
    table::Table,
    tables,
    transaction::{DbTx, DbTxMut},
    Error,
};
use reth_primitives::{BlockHash, BlockNumber, TransitionId, TxNumber};

use crate::{DatabaseIntegrityError, StageError};

/// A container for any DB transaction that will open a new inner transaction when the current
/// one is committed.
// NOTE: This container is needed since `Transaction::commit` takes `mut self`, so methods in
// the pipeline that just take a reference will not be able to commit their transaction and let
// the pipeline continue. Is there a better way to do this?
//
// TODO: Re-evaluate if this is actually needed, this was introduced as a way to manage the
// lifetime of the `TXMut` and having a nice API for re-opening a new transaction after `commit`
pub struct Transaction<'this, DB: Database> {
    /// A handle to the DB.
    pub(crate) db: &'this DB,
    tx: Option<<DB as DatabaseGAT<'this>>::TXMut>,
}

impl<'a, DB: Database> Debug for Transaction<'a, DB> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Transaction").finish()
    }
}

impl<'a, DB: Database> Deref for Transaction<'a, DB> {
    type Target = <DB as DatabaseGAT<'a>>::TXMut;

    /// Dereference as the inner transaction.
    ///
    /// # Panics
    ///
    /// Panics if an inner transaction does not exist. This should never be the case unless
    /// [Transaction::close] was called without following up with a call to [Transaction::open].
    fn deref(&self) -> &Self::Target {
        self.tx.as_ref().expect("Tried getting a reference to a non-existent transaction")
    }
}

impl<'a, DB: Database> DerefMut for Transaction<'a, DB> {
    /// Dereference as a mutable reference to the inner transaction.
    ///
    /// # Panics
    ///
    /// Panics if an inner transaction does not exist. This should never be the case unless
    /// [Transaction::close] was called without following up with a call to [Transaction::open].
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.tx.as_mut().expect("Tried getting a mutable reference to a non-existent transaction")
    }
}

impl<'this, DB> Transaction<'this, DB>
where
    DB: Database,
{
    /// Create a new container with the given database handle.
    ///
    /// A new inner transaction will be opened.
    pub fn new(db: &'this DB) -> Result<Self, Error> {
        Ok(Self { db, tx: Some(db.tx_mut()?) })
    }

    /// Accessor to the internal Database
    pub fn inner(&self) -> &'this DB {
        self.db
    }

    /// Commit the current inner transaction and open a new one.
    ///
    /// # Panics
    ///
    /// Panics if an inner transaction does not exist. This should never be the case unless
    /// [Transaction::close] was called without following up with a call to [Transaction::open].
    pub fn commit(&mut self) -> Result<bool, Error> {
        let success = if let Some(tx) = self.tx.take() { tx.commit()? } else { false };
        self.tx = Some(self.db.tx_mut()?);
        Ok(success)
    }

    /// Open a new inner transaction.
    pub fn open(&mut self) -> Result<(), Error> {
        self.tx = Some(self.db.tx_mut()?);
        Ok(())
    }

    /// Close the current inner transaction.
    pub fn close(&mut self) {
        self.tx.take();
    }

    /// Query [tables::CanonicalHeaders] table for block hash by block number
    pub(crate) fn get_block_hash(&self, number: BlockNumber) -> Result<BlockHash, StageError> {
        let hash = self
            .get::<tables::CanonicalHeaders>(number)?
            .ok_or(DatabaseIntegrityError::CanonicalHeader { number })?;
        Ok(hash)
    }

    /// Query for block hash by block number and return it as [BlockNumHash] key
    pub(crate) fn get_block_numhash(
        &self,
        number: BlockNumber,
    ) -> Result<BlockNumHash, StageError> {
        Ok((number, self.get_block_hash(number)?).into())
    }

    /// Query the block body by [BlockNumHash] key
    pub(crate) fn get_block_body(&self, key: BlockNumHash) -> Result<StoredBlockBody, StageError> {
        let body = self
            .get::<tables::BlockBodies>(key)?
            .ok_or(DatabaseIntegrityError::BlockBody { number: key.number() })?;
        Ok(body)
    }

    /// Query the block body by number
    pub(crate) fn get_block_body_by_num(
        &self,
        number: BlockNumber,
    ) -> Result<StoredBlockBody, StageError> {
        let key = self.get_block_numhash(number)?;
        self.get_block_body(key)
    }

    /// Query the last transition of the block by [BlockNumber] key
    pub(crate) fn get_block_transition(
        &self,
        key: BlockNumber,
    ) -> Result<TransitionId, StageError> {
        let last_transition_id = self
            .get::<tables::BlockTransitionIndex>(key)?
            .ok_or(DatabaseIntegrityError::BlockTransition { number: key })?;
        Ok(last_transition_id)
    }

    /// Get the next start transaction id and transition for the `block` by looking at the previous
    /// block. Returns Zero/Zero for Genesis.
    pub(crate) fn get_next_block_ids(
        &self,
        block: BlockNumber,
    ) -> Result<(TxNumber, TransitionId), StageError> {
        if block == 0 {
            return Ok((0, 0))
        }

        let prev_key = self.get_block_numhash(block - 1)?;
        let prev_body = self.get_block_body(prev_key)?;
        let last_transition = self
            .get::<tables::BlockTransitionIndex>(prev_key.number())?
            .ok_or(DatabaseIntegrityError::BlockTransition { number: prev_key.number() })?;
        Ok((prev_body.start_tx_id + prev_body.tx_count, last_transition))
    }

    /// Unwind table by some number key
    #[inline]
    pub(crate) fn unwind_table_by_num<T>(&self, num: u64) -> Result<(), Error>
    where
        DB: Database,
        T: Table<Key = u64>,
    {
        self.unwind_table::<T, _>(num, |key| key)
    }

    /// Unwind table by composite block number hash key
    #[inline]
    pub(crate) fn unwind_table_by_num_hash<T>(&self, block: BlockNumber) -> Result<(), Error>
    where
        DB: Database,
        T: Table<Key = BlockNumHash>,
    {
        self.unwind_table::<T, _>(block, |key| key.number())
    }

    /// Unwind the table to a provided block
    pub(crate) fn unwind_table<T, F>(
        &self,
        block: BlockNumber,
        mut selector: F,
    ) -> Result<(), Error>
    where
        DB: Database,
        T: Table,
        F: FnMut(T::Key) -> BlockNumber,
    {
        let mut cursor = self.cursor_write::<T>()?;
        let mut entry = cursor.last()?;
        while let Some((key, _)) = entry {
            if selector(key) <= block {
                break
            }
            cursor.delete_current()?;
            entry = cursor.prev()?;
        }
        Ok(())
    }

    /// Unwind a table forward by a [Walker] on another table
    pub(crate) fn unwind_table_by_walker<T1, T2>(&self, start_at: T1::Key) -> Result<(), Error>
    where
        DB: Database,
        T1: Table,
        T2: Table<Key = T1::Value>,
    {
        let mut cursor = self.cursor_write::<T1>()?;
        let mut walker = cursor.walk(start_at)?;
        while let Some((_, value)) = walker.next().transpose()? {
            self.delete::<T2>(value, None)?;
        }
        Ok(())
    }
}

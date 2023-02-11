#![allow(dead_code)]
use reth_db::{
    cursor::DbCursorRO,
    database::{Database, DatabaseGAT},
    models::StoredBlockBody,
    table::Table,
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_interfaces::{db::Error as DbError, provider::Error as ProviderError};
use reth_primitives::{BlockHash, BlockNumber, Header, TransitionId, TxNumber};
use std::{
    fmt::Debug,
    ops::{Deref, DerefMut},
};

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
    pub fn new(db: &'this DB) -> Result<Self, DbError> {
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
    pub fn commit(&mut self) -> Result<bool, DbError> {
        let success = if let Some(tx) = self.tx.take() { tx.commit()? } else { false };
        self.tx = Some(self.db.tx_mut()?);
        Ok(success)
    }

    /// Open a new inner transaction.
    pub fn open(&mut self) -> Result<(), DbError> {
        self.tx = Some(self.db.tx_mut()?);
        Ok(())
    }

    /// Close the current inner transaction.
    pub fn close(&mut self) {
        self.tx.take();
    }

    /// Query [tables::CanonicalHeaders] table for block hash by block number
    pub(crate) fn get_block_hash(
        &self,
        block_number: BlockNumber,
    ) -> Result<BlockHash, TransactionError> {
        let hash = self
            .get::<tables::CanonicalHeaders>(block_number)?
            .ok_or(ProviderError::CanonicalHeader { block_number })?;
        Ok(hash)
    }

    /// Query the block body by number.
    pub fn get_block_body(&self, number: BlockNumber) -> Result<StoredBlockBody, TransactionError> {
        let body =
            self.get::<tables::BlockBodies>(number)?.ok_or(ProviderError::BlockBody { number })?;
        Ok(body)
    }

    /// Query the last transition of the block by [BlockNumber] key
    pub fn get_block_transition(&self, key: BlockNumber) -> Result<TransitionId, TransactionError> {
        let last_transition_id = self
            .get::<tables::BlockTransitionIndex>(key)?
            .ok_or(ProviderError::BlockTransition { block_number: key })?;
        Ok(last_transition_id)
    }

    /// Get the next start transaction id and transition for the `block` by looking at the previous
    /// block. Returns Zero/Zero for Genesis.
    pub fn get_next_block_ids(
        &self,
        block: BlockNumber,
    ) -> Result<(TxNumber, TransitionId), TransactionError> {
        if block == 0 {
            return Ok((0, 0))
        }

        let prev_number = block - 1;
        let prev_body = self.get_block_body(prev_number)?;
        let last_transition = self
            .get::<tables::BlockTransitionIndex>(prev_number)?
            .ok_or(ProviderError::BlockTransition { block_number: prev_number })?;
        Ok((prev_body.start_tx_id + prev_body.tx_count, last_transition))
    }

    /// Query the block header by number
    pub fn get_header(&self, number: BlockNumber) -> Result<Header, TransactionError> {
        let header =
            self.get::<tables::Headers>(number)?.ok_or(ProviderError::Header { number })?;
        Ok(header)
    }

    /// Unwind table by some number key
    #[inline]
    pub fn unwind_table_by_num<T>(&self, num: u64) -> Result<(), DbError>
    where
        DB: Database,
        T: Table<Key = u64>,
    {
        self.unwind_table::<T, _>(num, |key| key)
    }

    /// Unwind the table to a provided block
    pub(crate) fn unwind_table<T, F>(
        &self,
        block: BlockNumber,
        mut selector: F,
    ) -> Result<(), DbError>
    where
        DB: Database,
        T: Table,
        F: FnMut(T::Key) -> BlockNumber,
    {
        let mut cursor = self.cursor_write::<T>()?;
        let mut reverse_walker = cursor.walk_back(None)?;

        while let Some(Ok((key, _))) = reverse_walker.next() {
            if selector(key.clone()) <= block {
                break
            }
            self.delete::<T>(key, None)?;
        }
        Ok(())
    }

    /// Unwind a table forward by a [Walker][reth_db::abstraction::cursor::Walker] on another table
    pub fn unwind_table_by_walker<T1, T2>(&self, start_at: T1::Key) -> Result<(), DbError>
    where
        DB: Database,
        T1: Table,
        T2: Table<Key = T1::Value>,
    {
        let mut cursor = self.cursor_write::<T1>()?;
        let mut walker = cursor.walk(Some(start_at))?;
        while let Some((_, value)) = walker.next().transpose()? {
            self.delete::<T2>(value, None)?;
        }
        Ok(())
    }
}

/// An error that can occur when using the transaction container
#[derive(Debug, thiserror::Error)]
pub enum TransactionError {
    /// The transaction encountered a database error.
    #[error("Database error: {0}")]
    Database(#[from] DbError),
    /// The transaction encountered a database integrity error.
    #[error("A database integrity error occurred: {0}")]
    DatabaseIntegrity(#[from] ProviderError),
}

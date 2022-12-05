use std::{
    fmt::Debug,
    ops::{Deref, DerefMut},
};

use reth_db::kv::cursor::PairResult;
use reth_interfaces::db::{
    models::{BlockNumHash, NumTransactions},
    tables, Database, DatabaseGAT, DbCursorRO, DbCursorRW, DbTx, DbTxMut, Error, Table,
};
use reth_primitives::{BlockHash, BlockNumber};

use crate::{DatabaseIntegrityError, StageError};

/// A container for any DB transaction that will open a new inner transaction when the current
/// one is committed.
// NOTE: This container is needed since `Transaction::commit` takes `mut self`, so methods in
// the pipeline that just take a reference will not be able to commit their transaction and let
// the pipeline continue. Is there a better way to do this?
pub struct StageDB<'this, DB: Database> {
    /// A handle to the DB.
    pub(crate) db: &'this DB,
    tx: Option<<DB as DatabaseGAT<'this>>::TXMut>,
}

impl<'a, DB: Database> Debug for StageDB<'a, DB> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StageDB").finish()
    }
}

impl<'a, DB: Database> Deref for StageDB<'a, DB> {
    type Target = <DB as DatabaseGAT<'a>>::TXMut;

    /// Dereference as the inner transaction.
    ///
    /// # Panics
    ///
    /// Panics if an inner transaction does not exist. This should never be the case unless
    /// [StageDB::close] was called without following up with a call to [StageDB::open].
    fn deref(&self) -> &Self::Target {
        self.tx.as_ref().expect("Tried getting a reference to a non-existent transaction")
    }
}

impl<'a, DB: Database> DerefMut for StageDB<'a, DB> {
    /// Dereference as a mutable reference to the inner transaction.
    ///
    /// # Panics
    ///
    /// Panics if an inner transaction does not exist. This should never be the case unless
    /// [StageDB::close] was called without following up with a call to [StageDB::open].
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.tx.as_mut().expect("Tried getting a mutable reference to a non-existent transaction")
    }
}

impl<'this, DB> StageDB<'this, DB>
where
    DB: Database,
{
    /// Create a new container with the given database handle.
    ///
    /// A new inner transaction will be opened.
    pub fn new(db: &'this DB) -> Result<Self, Error> {
        Ok(Self { db, tx: Some(db.tx_mut()?) })
    }

    /// Commit the current inner transaction and open a new one.
    ///
    /// # Panics
    ///
    /// Panics if an inner transaction does not exist. This should never be the case unless
    /// [StageDB::close] was called without following up with a call to [StageDB::open].
    pub fn commit(&mut self) -> Result<bool, Error> {
        let success =
            self.tx.take().expect("Tried committing a non-existent transaction").commit()?;
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

    /// Get exact or previous value from the database
    pub(crate) fn get_exact_or_prev<T: Table>(&self, key: T::Key) -> PairResult<T> {
        let mut cursor = self.cursor::<T>()?;
        Ok(cursor.seek_exact(key)?.or(cursor.prev()?))
    }

    /// Query [tables::CanonicalHeaders] table for block hash by block number
    pub(crate) fn get_block_hash(&self, number: BlockNumber) -> Result<BlockHash, StageError> {
        let hash = self
            .get::<tables::CanonicalHeaders>(number)?
            .ok_or(DatabaseIntegrityError::CanonicalHash { number })?;
        Ok(hash)
    }

    /// Query for block hash by block number and return it as [BlockNumHash] key
    pub(crate) fn get_block_numhash(
        &self,
        number: BlockNumber,
    ) -> Result<BlockNumHash, StageError> {
        Ok((number, self.get_block_hash(number)?).into())
    }

    /// Query [tables::CumulativeTxCount] table for total transaction
    /// count block by [BlockNumHash] key
    pub(crate) fn get_tx_count(&self, key: BlockNumHash) -> Result<NumTransactions, StageError> {
        let count = self.get::<tables::CumulativeTxCount>(key)?.ok_or(
            DatabaseIntegrityError::CumulativeTxCount { number: key.number(), hash: key.hash() },
        )?;
        Ok(count)
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
        let mut cursor = self.cursor_mut::<T>()?;
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
        let mut cursor = self.cursor_mut::<T1>()?;
        let mut walker = cursor.walk(start_at)?;
        while let Some((_, value)) = walker.next().transpose()? {
            self.delete::<T2>(value, None)?;
        }
        Ok(())
    }
}

use std::{
    fmt::Debug,
    ops::{Deref, DerefMut},
};

use reth_interfaces::db::{
    models::{BlockNumHash, NumTransactions},
    tables, DBContainer, Database, DatabaseGAT, DbCursorRO, DbCursorRW, DbTx, DbTxMut, Error,
    Table,
};
use reth_primitives::{BlockHash, BlockNumber};

use crate::{DatabaseIntegrityError, StageError};

/// A wrapper around [DBContainer] with utility methods
/// for querying and storing data during staged sync.
pub struct StageDB<'a, DB: Database>(DBContainer<'a, DB>);

impl<'a, DB: Database> Debug for StageDB<'a, DB> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StageDB").finish()
    }
}

impl<'a, DB: Database> Deref for StageDB<'a, DB> {
    type Target = <DB as DatabaseGAT<'a>>::TXMut;

    fn deref(&self) -> &Self::Target {
        self.0.get()
    }
}

impl<'a, DB: Database> DerefMut for StageDB<'a, DB> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.get_mut()
    }
}

impl<'a, DB: Database> StageDB<'a, DB> {
    /// Create new instance
    pub(crate) fn new(db: &'a DB) -> Result<Self, Error> {
        Ok(Self(DBContainer::new(db)?))
    }

    /// Commit the underlying transaction
    pub(crate) fn commit(&mut self) -> Result<bool, Error> {
        self.0.commit()
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

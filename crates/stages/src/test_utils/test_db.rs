use reth_db::{
    cursor::{DbCursorRO, DbCursorRW},
    mdbx::{test_utils::create_test_db, tx::Tx, Env, EnvKind, WriteMap, RW},
    models::BlockNumHash,
    table::Table,
    tables,
    transaction::{DbTx, DbTxMut},
    Error as DbError,
};
use reth_primitives::{BlockNumber, SealedHeader, U256};
use std::{borrow::Borrow, sync::Arc};

use crate::db::StageDB;

/// The [StageTestDB] is used as an internal
/// database for testing stage implementation.
///
/// ```rust
/// let db = StageTestDB::default();
/// stage.execute(&mut db.container(), input);
/// ```
pub(crate) struct TestStageDB {
    db: Arc<Env<WriteMap>>,
}

impl Default for TestStageDB {
    /// Create a new instance of [StageTestDB]
    fn default() -> Self {
        Self { db: create_test_db::<WriteMap>(EnvKind::RW) }
    }
}

impl TestStageDB {
    /// Return a database wrapped in [StageDB].
    pub(crate) fn inner(&self) -> StageDB<'_, Env<WriteMap>> {
        StageDB::new(self.db.borrow()).expect("failed to create db container")
    }

    /// Get a pointer to an internal database.
    pub(crate) fn inner_raw(&self) -> Arc<Env<WriteMap>> {
        self.db.clone()
    }

    /// Invoke a callback with transaction committing it afterwards
    pub(crate) fn commit<F>(&self, f: F) -> Result<(), DbError>
    where
        F: FnOnce(&mut Tx<'_, RW, WriteMap>) -> Result<(), DbError>,
    {
        let mut db = self.inner();
        f(&mut db)?;
        db.commit()?;
        Ok(())
    }

    /// Invoke a callback with a read transaction
    pub(crate) fn query<F, R>(&self, f: F) -> Result<R, DbError>
    where
        F: FnOnce(&Tx<'_, RW, WriteMap>) -> Result<R, DbError>,
    {
        f(&self.inner())
    }

    /// Check if the table is empty
    pub(crate) fn table_is_empty<T: Table>(&self) -> Result<bool, DbError> {
        self.query(|tx| {
            let last = tx.cursor::<T>()?.last()?;
            Ok(last.is_none())
        })
    }

    /// Map a collection of values and store them in the database.
    /// This function commits the transaction before exiting.
    ///
    /// ```rust
    /// let db = StageTestDB::default();
    /// db.map_put::<Table, _, _>(&items, |item| item)?;
    /// ```
    #[allow(dead_code)]
    pub(crate) fn map_put<T, S, F>(&self, values: &[S], mut map: F) -> Result<(), DbError>
    where
        T: Table,
        S: Clone,
        F: FnMut(&S) -> (T::Key, T::Value),
    {
        self.commit(|tx| {
            values.iter().try_for_each(|src| {
                let (k, v) = map(src);
                tx.put::<T>(k, v)
            })
        })
    }

    /// Transform a collection of values using a callback and store
    /// them in the database. The callback additionally accepts the
    /// optional last element that was stored.
    /// This function commits the transaction before exiting.
    ///
    /// ```rust
    /// let db = StageTestDB::default();
    /// db.transform_append::<Table, _, _>(&items, |prev, item| prev.unwrap_or_default() + item)?;
    /// ```
    #[allow(dead_code)]
    pub(crate) fn transform_append<T, S, F>(
        &self,
        values: &[S],
        mut transform: F,
    ) -> Result<(), DbError>
    where
        T: Table,
        <T as Table>::Value: Clone,
        S: Clone,
        F: FnMut(&Option<<T as Table>::Value>, &S) -> (T::Key, T::Value),
    {
        self.commit(|tx| {
            let mut cursor = tx.cursor_mut::<T>()?;
            let mut last = cursor.last()?.map(|(_, v)| v);
            values.iter().try_for_each(|src| {
                let (k, v) = transform(&last, src);
                last = Some(v.clone());
                cursor.append(k, v)
            })
        })
    }

    /// Check that there is no table entry above a given
    /// number by [Table::Key]
    pub(crate) fn check_no_entry_above<T, F>(
        &self,
        num: u64,
        mut selector: F,
    ) -> Result<(), DbError>
    where
        T: Table,
        F: FnMut(T::Key) -> BlockNumber,
    {
        self.query(|tx| {
            let mut cursor = tx.cursor::<T>()?;
            if let Some((key, _)) = cursor.last()? {
                assert!(selector(key) <= num);
            }
            Ok(())
        })
    }

    /// Check that there is no table entry above a given
    /// number by [Table::Value]
    pub(crate) fn check_no_entry_above_by_value<T, F>(
        &self,
        num: u64,
        mut selector: F,
    ) -> Result<(), DbError>
    where
        T: Table,
        F: FnMut(T::Value) -> BlockNumber,
    {
        self.query(|tx| {
            let mut cursor = tx.cursor::<T>()?;
            let mut entry = cursor.last()?;
            while let Some((_, value)) = entry {
                assert!(selector(value) <= num);
                entry = cursor.prev()?;
            }
            Ok(())
        })
    }

    /// Insert ordered collection of [SealedHeader] into the corresponding tables
    /// that are supposed to be populated by the headers stage.
    pub(crate) fn insert_headers<'a, I>(&self, headers: I) -> Result<(), DbError>
    where
        I: Iterator<Item = &'a SealedHeader>,
    {
        self.commit(|tx| {
            let headers = headers.collect::<Vec<_>>();

            let mut td: U256 =
                tx.cursor::<tables::HeaderTD>()?.last()?.map(|(_, v)| v).unwrap_or_default().into();

            for header in headers {
                let key: BlockNumHash = (header.number, header.hash()).into();

                tx.put::<tables::CanonicalHeaders>(header.number, header.hash())?;
                tx.put::<tables::HeaderNumbers>(header.hash(), header.number)?;
                tx.put::<tables::Headers>(key, header.clone().unseal())?;

                td += header.difficulty;
                tx.put::<tables::HeaderTD>(key, td.into())?;
            }

            Ok(())
        })
    }
}

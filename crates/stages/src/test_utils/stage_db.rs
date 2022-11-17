use reth_db::{
    kv::{test_utils::create_test_db, tx::Tx, Env, EnvKind},
    mdbx::{WriteMap, RW},
};
use reth_interfaces::db::{self, DBContainer, DbCursorRO, DbCursorRW, DbTx, DbTxMut, Table};
use reth_primitives::BlockNumber;
use std::{borrow::Borrow, sync::Arc};

/// The [StageTestDB] is used as an internal
/// database for testing stage implementation.
///
/// ```rust
/// let db = StageTestDB::default();
/// stage.execute(&mut db.container(), input);
/// ```
pub(crate) struct StageTestDB {
    db: Arc<Env<WriteMap>>,
}

impl Default for StageTestDB {
    /// Create a new instance of [StageTestDB]
    fn default() -> Self {
        Self { db: create_test_db::<WriteMap>(EnvKind::RW) }
    }
}

impl StageTestDB {
    /// Return a database wrapped in [DBContainer].
    fn container(&self) -> DBContainer<'_, Env<WriteMap>> {
        DBContainer::new(self.db.borrow()).expect("failed to create db container")
    }

    /// Get a pointer to an internal database.
    pub(crate) fn inner(&self) -> Arc<Env<WriteMap>> {
        self.db.clone()
    }

    /// Invoke a callback with transaction committing it afterwards
    pub(crate) fn commit<F>(&self, f: F) -> Result<(), db::Error>
    where
        F: FnOnce(&mut Tx<'_, RW, WriteMap>) -> Result<(), db::Error>,
    {
        let mut db = self.container();
        let tx = db.get_mut();
        f(tx)?;
        db.commit()?;
        Ok(())
    }

    /// Invoke a callback with a read transaction
    pub(crate) fn query<F, R>(&self, f: F) -> Result<R, db::Error>
    where
        F: FnOnce(&Tx<'_, RW, WriteMap>) -> Result<R, db::Error>,
    {
        f(self.container().get())
    }

    /// Map a collection of values and store them in the database.
    /// This function commits the transaction before exiting.
    ///
    /// ```rust
    /// let db = StageTestDB::default();
    /// db.map_put::<Table, _, _>(&items, |item| item)?;
    /// ```
    pub(crate) fn map_put<T, S, F>(&self, values: &[S], mut map: F) -> Result<(), db::Error>
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
    pub(crate) fn transform_append<T, S, F>(
        &self,
        values: &[S],
        mut transform: F,
    ) -> Result<(), db::Error>
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
    /// block by [Table::Key]
    pub(crate) fn check_no_entry_above<T, F>(
        &self,
        block: BlockNumber,
        mut selector: F,
    ) -> Result<(), db::Error>
    where
        T: Table,
        F: FnMut(T::Key) -> BlockNumber,
    {
        self.query(|tx| {
            let mut cursor = tx.cursor::<T>()?;
            if let Some((key, _)) = cursor.last()? {
                assert!(selector(key) <= block);
            }
            Ok(())
        })
    }

    /// Check that there is no table entry above a given
    /// block by [Table::Value]
    pub(crate) fn check_no_entry_above_by_value<T, F>(
        &self,
        block: BlockNumber,
        mut selector: F,
    ) -> Result<(), db::Error>
    where
        T: Table,
        F: FnMut(T::Value) -> BlockNumber,
    {
        self.query(|tx| {
            let mut cursor = tx.cursor::<T>()?;
            if let Some((_, value)) = cursor.last()? {
                assert!(selector(value) <= block);
            }
            Ok(())
        })
    }
}

pub(crate) mod opt {
    use tokio::sync::mpsc::{error::SendError, Sender};

    /// Get an [Option] with the maximum value, compared between the passed in value and the inner
    /// value of the [Option]. If the [Option] is `None`, then an option containing the passed in
    /// value will be returned.
    pub(crate) fn max<T: Ord + Copy>(a: Option<T>, b: T) -> Option<T> {
        a.map_or(Some(b), |v| Some(std::cmp::max(v, b)))
    }

    /// Get an [Option] with the minimum value, compared between the passed in value and the inner
    /// value of the [Option]. If the [Option] is `None`, then an option containing the passed in
    /// value will be returned.
    pub(crate) fn min<T: Ord + Copy>(a: Option<T>, b: T) -> Option<T> {
        a.map_or(Some(b), |v| Some(std::cmp::min(v, b)))
    }

    /// The producing side of a [tokio::mpsc] channel that may or may not be set.
    #[derive(Default, Clone)]
    pub(crate) struct MaybeSender<T> {
        inner: Option<Sender<T>>,
    }

    impl<T> MaybeSender<T> {
        /// Create a new [MaybeSender]
        pub(crate) fn new(sender: Option<Sender<T>>) -> Self {
            Self { inner: sender }
        }

        /// Send a value over the channel if an internal sender has been set.
        pub(crate) async fn send(&self, value: T) -> Result<(), SendError<T>> {
            if let Some(rx) = &self.inner {
                rx.send(value).await
            } else {
                Ok(())
            }
        }

        /// Set or unset the internal sender.
        pub(crate) fn set(&mut self, sender: Option<Sender<T>>) {
            self.inner = sender;
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn opt_max() {
            assert_eq!(max(None, 5), Some(5));
            assert_eq!(max(Some(1), 5), Some(5));
            assert_eq!(max(Some(10), 5), Some(10));
        }

        #[test]
        fn opt_min() {
            assert_eq!(min(None, 5), Some(5));
            assert_eq!(min(Some(1), 5), Some(1));
            assert_eq!(min(Some(10), 5), Some(5));
        }
    }
}

pub(crate) mod unwind {
    use reth_interfaces::db::{
        models::BlockNumHash, Database, DatabaseGAT, DbCursorRO, DbCursorRW, DbTxMut, Error, Table,
    };
    use reth_primitives::BlockNumber;

    /// Unwind table by block number key
    #[inline]
    pub(crate) fn unwind_table_by_num<DB, T>(
        tx: &mut <DB as DatabaseGAT<'_>>::TXMut,
        block: BlockNumber,
    ) -> Result<(), Error>
    where
        DB: Database,
        T: Table<Key = BlockNumber>,
    {
        unwind_table::<DB, T, _>(tx, block, |key| key)
    }

    /// Unwind table by composite block number hash key
    #[inline]
    pub(crate) fn unwind_table_by_num_hash<DB, T>(
        tx: &mut <DB as DatabaseGAT<'_>>::TXMut,
        block: BlockNumber,
    ) -> Result<(), Error>
    where
        DB: Database,
        T: Table<Key = BlockNumHash>,
    {
        unwind_table::<DB, T, _>(tx, block, |key| key.number())
    }

    /// Unwind the table to a provided block
    pub(crate) fn unwind_table<DB, T, F>(
        tx: &mut <DB as DatabaseGAT<'_>>::TXMut,
        block: BlockNumber,
        mut selector: F,
    ) -> Result<(), Error>
    where
        DB: Database,
        T: Table,
        F: FnMut(T::Key) -> BlockNumber,
    {
        let mut cursor = tx.cursor_mut::<T>()?;
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
}

#[cfg(test)]
pub(crate) mod test_utils {
    use reth_db::{
        kv::{test_utils::create_test_db, Env, EnvKind},
        mdbx::WriteMap,
    };
    use reth_interfaces::db::{DBContainer, DbCursorRO, DbCursorRW, DbTx, DbTxMut, Error, Table};
    use reth_primitives::BlockNumber;
    use std::{borrow::Borrow, sync::Arc};
    use tokio::sync::oneshot;

    use crate::{ExecInput, ExecOutput, Stage, StageError, UnwindInput, UnwindOutput};

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
        /// Get a pointer to an internal database.
        pub(crate) fn inner(&self) -> Arc<Env<WriteMap>> {
            self.db.clone()
        }

        /// Return a database wrapped in [DBContainer].
        pub(crate) fn container(&self) -> DBContainer<'_, Env<WriteMap>> {
            DBContainer::new(self.db.borrow()).expect("failed to create db container")
        }

        /// Map a collection of values and store them in the database.
        /// This function commits the transaction before exiting.
        ///
        /// ```rust
        /// let db = StageTestDB::default();
        /// db.map_put::<Table, _, _>(&items, |item| item)?;
        /// ```
        pub(crate) fn map_put<T, S, F>(&self, values: &[S], mut map: F) -> Result<(), Error>
        where
            T: Table,
            S: Clone,
            F: FnMut(&S) -> (T::Key, T::Value),
        {
            let mut db = self.container();
            let tx = db.get_mut();
            values.iter().try_for_each(|src| {
                let (k, v) = map(src);
                tx.put::<T>(k, v)
            })?;
            db.commit()?;
            Ok(())
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
        ) -> Result<(), Error>
        where
            T: Table,
            <T as Table>::Value: Clone,
            S: Clone,
            F: FnMut(&Option<<T as Table>::Value>, &S) -> (T::Key, T::Value),
        {
            let mut db = self.container();
            let tx = db.get_mut();
            let mut cursor = tx.cursor_mut::<T>()?;
            let mut last = cursor.last()?.map(|(_, v)| v);
            values.iter().try_for_each(|src| {
                let (k, v) = transform(&last, src);
                last = Some(v.clone());
                cursor.append(k, v)
            })?;
            db.commit()?;
            Ok(())
        }

        /// Check there there is no table entry above a given block
        pub(crate) fn check_no_entry_above<T: Table, F>(
            &self,
            block: BlockNumber,
            mut selector: F,
        ) -> Result<(), Error>
        where
            T: Table,
            F: FnMut(T::Key) -> BlockNumber,
        {
            let db = self.container();
            let tx = db.get();

            let mut cursor = tx.cursor::<T>()?;
            if let Some((key, _)) = cursor.last()? {
                assert!(selector(key) <= block);
            }

            Ok(())
        }
    }

    /// A generic test runner for stages.
    #[async_trait::async_trait]
    pub(crate) trait StageTestRunner {
        type S: Stage<Env<WriteMap>> + 'static;

        /// Return a reference to the database.
        fn db(&self) -> &StageTestDB;

        /// Return an instance of a Stage.
        fn stage(&self) -> Self::S;

        /// Run [Stage::execute] and return a receiver for the result.
        fn execute(&self, input: ExecInput) -> oneshot::Receiver<Result<ExecOutput, StageError>> {
            let (tx, rx) = oneshot::channel();
            let (db, mut stage) = (self.db().inner(), self.stage());
            tokio::spawn(async move {
                let mut db = DBContainer::new(db.borrow()).expect("failed to create db container");
                let result = stage.execute(&mut db, input).await;
                db.commit().expect("failed to commit");
                tx.send(result).expect("failed to send message")
            });
            rx
        }

        /// Run [Stage::unwind] and return a receiver for the result.
        fn unwind(
            &self,
            input: UnwindInput,
        ) -> oneshot::Receiver<Result<UnwindOutput, Box<dyn std::error::Error + Send + Sync>>>
        {
            let (tx, rx) = oneshot::channel();
            let (db, mut stage) = (self.db().inner(), self.stage());
            tokio::spawn(async move {
                let mut db = DBContainer::new(db.borrow()).expect("failed to create db container");
                let result = stage.unwind(&mut db, input).await;
                db.commit().expect("failed to commit");
                tx.send(result).expect("failed to send result");
            });
            rx
        }
    }
}

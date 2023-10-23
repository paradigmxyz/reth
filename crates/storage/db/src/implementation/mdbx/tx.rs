//! Transaction wrapper for libmdbx-sys.

use super::cursor::Cursor;
use crate::{
    metrics::{MetricEvent, MetricEventsSender, Operation, TransactionOutcome},
    table::{Compress, DupSort, Encode, Table, TableImporter},
    tables::{utils::decode_one, Tables, NUM_TABLES},
    transaction::{DbTx, DbTxGAT, DbTxMut, DbTxMutGAT},
    DatabaseError,
};
use parking_lot::RwLock;
use reth_interfaces::db::DatabaseWriteOperation;
use reth_libmdbx::{ffi::DBI, EnvironmentKind, Transaction, TransactionKind, WriteFlags, RW};
use std::{str::FromStr, sync::Arc, time::Instant};

/// Wrapper for the libmdbx transaction.
#[derive(Debug)]
pub struct Tx<'a, K: TransactionKind, E: EnvironmentKind> {
    /// Libmdbx-sys transaction.
    pub inner: Transaction<'a, K, E>,
    /// Cached internal transaction ID provided by libmdbx.
    id: Option<u64>,
    /// Database table handle cache.
    pub(crate) db_handles: Arc<RwLock<[Option<DBI>; NUM_TABLES]>>,
    metrics_tx: Option<MetricEventsSender>,
}

impl<'env, K: TransactionKind, E: EnvironmentKind> Tx<'env, K, E> {
    /// Creates new `Tx` object with a `RO` or `RW` transaction.
    pub fn new<'a>(inner: Transaction<'a, K, E>) -> Self
    where
        'a: 'env,
    {
        Self { inner, id: None, db_handles: Default::default(), metrics_tx: None }
    }

    /// Sets the [MetricEventsSender] to report metrics about the transaction and cursors.
    pub fn with_metrics_tx(mut self, metrics_tx: MetricEventsSender) -> Self {
        self.metrics_tx = Some(metrics_tx);
        self
    }

    /// Gets this transaction ID.
    pub fn id(&mut self) -> u64 {
        *self.id.get_or_insert_with(|| self.inner.id())
    }

    /// Gets a table database handle if it exists, otherwise creates it.
    pub fn get_dbi<T: Table>(&self) -> Result<DBI, DatabaseError> {
        let mut handles = self.db_handles.write();

        let table = Tables::from_str(T::NAME).expect("Requested table should be part of `Tables`.");

        let dbi_handle = handles.get_mut(table as usize).expect("should exist");
        if dbi_handle.is_none() {
            *dbi_handle = Some(
                self.inner
                    .open_db(Some(T::NAME))
                    .map_err(|e| DatabaseError::InitCursor(e.into()))?
                    .dbi(),
            );
        }

        Ok(dbi_handle.expect("is some; qed"))
    }

    /// Create db Cursor
    pub fn new_cursor<T: Table>(&self) -> Result<Cursor<'env, K, T>, DatabaseError> {
        Ok(Cursor::new(
            self.inner
                .cursor_with_dbi(self.get_dbi::<T>()?)
                .map_err(|e| DatabaseError::InitCursor(e.into()))?,
        ))
    }

    fn execute_with_operation_metric<T>(
        &self,
        operation: Operation,
        f: impl FnOnce(&Transaction<'_, K, E>) -> Result<T, DatabaseError>,
    ) -> Result<T, DatabaseError> {
        let start = Instant::now();
        let result = f(&self.inner);
        if let Some(metrics_tx) = &self.metrics_tx {
            let _ =
                metrics_tx.send(MetricEvent::Operation { operation, duration: start.elapsed() });
        }
        result
    }
}

impl<'a, K: TransactionKind, E: EnvironmentKind> DbTxGAT<'a> for Tx<'_, K, E> {
    type Cursor<T: Table> = Cursor<'a, K, T>;
    type DupCursor<T: DupSort> = Cursor<'a, K, T>;
}

impl<'a, K: TransactionKind, E: EnvironmentKind> DbTxMutGAT<'a> for Tx<'_, K, E> {
    type CursorMut<T: Table> = Cursor<'a, RW, T>;
    type DupCursorMut<T: DupSort> = Cursor<'a, RW, T>;
}

impl<E: EnvironmentKind> TableImporter for Tx<'_, RW, E> {}

impl<K: TransactionKind, E: EnvironmentKind> DbTx for Tx<'_, K, E> {
    fn get<T: Table>(&self, key: T::Key) -> Result<Option<<T as Table>::Value>, DatabaseError> {
        self.execute_with_operation_metric(Operation::Get, |tx| {
            tx.get(self.get_dbi::<T>()?, key.encode().as_ref())
                .map_err(|e| DatabaseError::Read(e.into()))?
                .map(decode_one::<T>)
                .transpose()
        })
    }

    fn commit(mut self) -> Result<bool, DatabaseError> {
        let start = Instant::now();
        let metrics = self.metrics_tx.take().map(|metrics_tx| (self.id(), metrics_tx));
        let result = self.inner.commit().map_err(|e| DatabaseError::Commit(e.into()));
        if let Some((txn_id, metrics_tx)) = metrics {
            let _ = metrics_tx.send(MetricEvent::CloseTransaction {
                txn_id,
                outcome: TransactionOutcome::Commit,
                commit_duration: start.elapsed(),
            });
        }
        result
    }

    fn drop(mut self) {
        let start = Instant::now();
        let metrics = self.metrics_tx.take().map(|metrics_tx| (self.id(), metrics_tx));
        drop(self.inner);
        if let Some((txn_id, metrics_tx)) = metrics {
            let _ = metrics_tx.send(MetricEvent::CloseTransaction {
                txn_id,
                outcome: TransactionOutcome::Abort,
                commit_duration: start.elapsed(),
            });
        }
    }

    // Iterate over read only values in database.
    fn cursor_read<T: Table>(&self) -> Result<<Self as DbTxGAT<'_>>::Cursor<T>, DatabaseError> {
        let mut cursor = self.new_cursor()?;
        if let Some(metrics_tx) = &self.metrics_tx {
            cursor = cursor.with_metrics_tx(metrics_tx.clone());
        }
        Ok(cursor)
    }

    /// Iterate over read only values in database.
    fn cursor_dup_read<T: DupSort>(
        &self,
    ) -> Result<<Self as DbTxGAT<'_>>::DupCursor<T>, DatabaseError> {
        let mut cursor = self.new_cursor()?;
        if let Some(metrics_tx) = &self.metrics_tx {
            cursor = cursor.with_metrics_tx(metrics_tx.clone());
        }
        Ok(cursor)
    }

    /// Returns number of entries in the table using cheap DB stats invocation.
    fn entries<T: Table>(&self) -> Result<usize, DatabaseError> {
        Ok(self
            .inner
            .db_stat_with_dbi(self.get_dbi::<T>()?)
            .map_err(|e| DatabaseError::Stats(e.into()))?
            .entries())
    }
}

impl<E: EnvironmentKind> DbTxMut for Tx<'_, RW, E> {
    fn put<T: Table>(&self, key: T::Key, value: T::Value) -> Result<(), DatabaseError> {
        let key = key.encode();
        self.execute_with_operation_metric(Operation::Put, |tx| {
            tx.put(self.get_dbi::<T>()?, key.as_ref(), &value.compress(), WriteFlags::UPSERT)
                .map_err(|e| DatabaseError::Write {
                    code: e.into(),
                    operation: DatabaseWriteOperation::Put,
                    table_name: T::NAME,
                    key: Box::from(key.as_ref()),
                })
        })
    }

    fn delete<T: Table>(
        &self,
        key: T::Key,
        value: Option<T::Value>,
    ) -> Result<bool, DatabaseError> {
        let mut data = None;

        let value = value.map(Compress::compress);
        if let Some(value) = &value {
            data = Some(value.as_ref());
        };

        self.execute_with_operation_metric(Operation::Delete, |tx| {
            tx.del(self.get_dbi::<T>()?, key.encode(), data)
                .map_err(|e| DatabaseError::Delete(e.into()))
        })
    }

    fn clear<T: Table>(&self) -> Result<(), DatabaseError> {
        self.inner.clear_db(self.get_dbi::<T>()?).map_err(|e| DatabaseError::Delete(e.into()))?;

        Ok(())
    }

    fn cursor_write<T: Table>(
        &self,
    ) -> Result<<Self as DbTxMutGAT<'_>>::CursorMut<T>, DatabaseError> {
        let mut cursor = self.new_cursor()?;
        if let Some(metrics_tx) = &self.metrics_tx {
            cursor = cursor.with_metrics_tx(metrics_tx.clone());
        }
        Ok(cursor)
    }

    fn cursor_dup_write<T: DupSort>(
        &self,
    ) -> Result<<Self as DbTxMutGAT<'_>>::DupCursorMut<T>, DatabaseError> {
        let mut cursor = self.new_cursor()?;
        if let Some(metrics_tx) = &self.metrics_tx {
            cursor = cursor.with_metrics_tx(metrics_tx.clone());
        }
        Ok(cursor)
    }
}

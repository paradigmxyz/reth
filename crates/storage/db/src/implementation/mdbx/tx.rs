//! Transaction wrapper for libmdbx-sys.

use super::cursor::Cursor;
use crate::{
    metrics::{DatabaseEnvMetrics, Operation, TransactionMode, TransactionOutcome},
    tables::utils::decode_one,
    DatabaseError,
};
use reth_db_api::{
    table::{Compress, DupSort, Encode, Table, TableImporter},
    transaction::{DbTx, DbTxMut},
};
use reth_libmdbx::{ffi::MDBX_dbi, CommitLatency, Transaction, TransactionKind, WriteFlags, RW};
use reth_storage_errors::db::{DatabaseWriteError, DatabaseWriteOperation};
use reth_tracing::tracing::{debug, trace, warn};
use std::{
    backtrace::Backtrace,
    marker::PhantomData,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

/// Duration after which we emit the log about long-lived database transactions.
const LONG_TRANSACTION_DURATION: Duration = Duration::from_secs(60);

/// Wrapper for the libmdbx transaction.
#[derive(Debug)]
pub struct Tx<K: TransactionKind> {
    /// Libmdbx-sys transaction.
    pub inner: Transaction<K>,

    /// Handler for metrics with its own [Drop] implementation for cases when the transaction isn't
    /// closed by [`Tx::commit`] or [`Tx::abort`], but we still need to report it in the metrics.
    ///
    /// If [Some], then metrics are reported.
    metrics_handler: Option<MetricsHandler<K>>,
}

impl<K: TransactionKind> Tx<K> {
    /// Creates new `Tx` object with a `RO` or `RW` transaction.
    #[inline]
    pub const fn new(inner: Transaction<K>) -> Self {
        Self::new_inner(inner, None)
    }

    /// Creates new `Tx` object with a `RO` or `RW` transaction and optionally enables metrics.
    #[inline]
    #[track_caller]
    pub(crate) fn new_with_metrics(
        inner: Transaction<K>,
        env_metrics: Option<Arc<DatabaseEnvMetrics>>,
    ) -> reth_libmdbx::Result<Self> {
        let metrics_handler = env_metrics
            .map(|env_metrics| {
                let handler = MetricsHandler::<K>::new(inner.id()?, env_metrics);
                handler.env_metrics.record_opened_transaction(handler.transaction_mode());
                handler.log_transaction_opened();
                Ok(handler)
            })
            .transpose()?;
        Ok(Self::new_inner(inner, metrics_handler))
    }

    #[inline]
    const fn new_inner(inner: Transaction<K>, metrics_handler: Option<MetricsHandler<K>>) -> Self {
        Self { inner, metrics_handler }
    }

    /// Gets this transaction ID.
    pub fn id(&self) -> reth_libmdbx::Result<u64> {
        self.metrics_handler.as_ref().map_or_else(|| self.inner.id(), |handler| Ok(handler.txn_id))
    }

    /// Gets a table database handle if it exists, otherwise creates it.
    pub fn get_dbi<T: Table>(&self) -> Result<MDBX_dbi, DatabaseError> {
        self.inner
            .open_db(Some(T::NAME))
            .map(|db| db.dbi())
            .map_err(|e| DatabaseError::Open(e.into()))
    }

    /// Create db Cursor
    pub fn new_cursor<T: Table>(&self) -> Result<Cursor<K, T>, DatabaseError> {
        let inner = self
            .inner
            .cursor_with_dbi(self.get_dbi::<T>()?)
            .map_err(|e| DatabaseError::InitCursor(e.into()))?;

        Ok(Cursor::new_with_metrics(
            inner,
            self.metrics_handler.as_ref().map(|h| h.env_metrics.clone()),
        ))
    }

    /// If `self.metrics_handler == Some(_)`, measure the time it takes to execute the closure and
    /// record a metric with the provided transaction outcome.
    ///
    /// Otherwise, just execute the closure.
    fn execute_with_close_transaction_metric<R>(
        mut self,
        outcome: TransactionOutcome,
        f: impl FnOnce(Self) -> (R, Option<CommitLatency>),
    ) -> R {
        let run = |tx| {
            let start = Instant::now();
            let (result, commit_latency) = f(tx);
            let total_duration = start.elapsed();

            if outcome.is_commit() {
                debug!(
                    target: "storage::db::mdbx",
                    ?total_duration,
                    ?commit_latency,
                    is_read_only = K::IS_READ_ONLY,
                    "Commit"
                );
            }

            (result, commit_latency, total_duration)
        };

        if let Some(mut metrics_handler) = self.metrics_handler.take() {
            metrics_handler.close_recorded = true;
            metrics_handler.log_backtrace_on_long_read_transaction();

            let (result, commit_latency, close_duration) = run(self);
            let open_duration = metrics_handler.start.elapsed();
            metrics_handler.env_metrics.record_closed_transaction(
                metrics_handler.transaction_mode(),
                outcome,
                open_duration,
                Some(close_duration),
                commit_latency,
            );

            result
        } else {
            run(self).0
        }
    }

    /// If `self.metrics_handler == Some(_)`, measure the time it takes to execute the closure and
    /// record a metric with the provided operation.
    ///
    /// Otherwise, just execute the closure.
    fn execute_with_operation_metric<T: Table, R>(
        &self,
        operation: Operation,
        value_size: Option<usize>,
        f: impl FnOnce(&Transaction<K>) -> R,
    ) -> R {
        if let Some(metrics_handler) = &self.metrics_handler {
            metrics_handler.log_backtrace_on_long_read_transaction();
            metrics_handler
                .env_metrics
                .record_operation(T::NAME, operation, value_size, || f(&self.inner))
        } else {
            f(&self.inner)
        }
    }
}

#[derive(Debug)]
struct MetricsHandler<K: TransactionKind> {
    /// Cached internal transaction ID provided by libmdbx.
    txn_id: u64,
    /// The time when transaction has started.
    start: Instant,
    /// Duration after which we emit the log about long-lived database transactions.
    long_transaction_duration: Duration,
    /// If `true`, the metric about transaction closing has already been recorded and we don't need
    /// to do anything on [`Drop::drop`].
    close_recorded: bool,
    /// If `true`, the backtrace of transaction will be recorded and logged.
    /// See [`MetricsHandler::log_backtrace_on_long_read_transaction`].
    record_backtrace: bool,
    /// If `true`, the backtrace of transaction has already been recorded and logged.
    /// See [`MetricsHandler::log_backtrace_on_long_read_transaction`].
    backtrace_recorded: AtomicBool,
    /// Shared database environment metrics.
    env_metrics: Arc<DatabaseEnvMetrics>,
    /// Backtrace of the location where the transaction has been opened. Reported only with debug
    /// assertions, because capturing the backtrace on every transaction opening is expensive.
    #[cfg(debug_assertions)]
    open_backtrace: Backtrace,
    _marker: PhantomData<K>,
}

impl<K: TransactionKind> MetricsHandler<K> {
    fn new(txn_id: u64, env_metrics: Arc<DatabaseEnvMetrics>) -> Self {
        Self {
            txn_id,
            start: Instant::now(),
            long_transaction_duration: LONG_TRANSACTION_DURATION,
            close_recorded: false,
            record_backtrace: true,
            backtrace_recorded: AtomicBool::new(false),
            #[cfg(debug_assertions)]
            open_backtrace: Backtrace::force_capture(),
            env_metrics,
            _marker: PhantomData,
        }
    }

    const fn transaction_mode(&self) -> TransactionMode {
        if K::IS_READ_ONLY {
            TransactionMode::ReadOnly
        } else {
            TransactionMode::ReadWrite
        }
    }

    /// Logs the caller location and ID of the transaction that was opened.
    #[track_caller]
    fn log_transaction_opened(&self) {
        trace!(
            target: "storage::db::mdbx",
            caller = %core::panic::Location::caller(),
            id = %self.txn_id,
            mode = %self.transaction_mode().as_str(),
            "Transaction opened",
        );
    }

    /// Logs the backtrace of current call if the duration that the read transaction has been open
    /// is more than [`LONG_TRANSACTION_DURATION`] and `record_backtrace == true`.
    /// The backtrace is recorded and logged just once, guaranteed by `backtrace_recorded` atomic.
    ///
    /// NOTE: Backtrace is recorded using [`Backtrace::force_capture`], so `RUST_BACKTRACE` env var
    /// is not needed.
    fn log_backtrace_on_long_read_transaction(&self) {
        if self.record_backtrace &&
            !self.backtrace_recorded.load(Ordering::Relaxed) &&
            self.transaction_mode().is_read_only()
        {
            let open_duration = self.start.elapsed();
            if open_duration >= self.long_transaction_duration {
                self.backtrace_recorded.store(true, Ordering::Relaxed);
                #[cfg(debug_assertions)]
                let message = format!(
                   "The database read transaction has been open for too long. Open backtrace:\n{}\n\nCurrent backtrace:\n{}",
                   self.open_backtrace,
                   Backtrace::force_capture()
                );
                #[cfg(not(debug_assertions))]
                let message = format!(
                    "The database read transaction has been open for too long. Backtrace:\n{}",
                    Backtrace::force_capture()
                );
                warn!(
                    target: "storage::db::mdbx",
                    ?open_duration,
                    %self.txn_id,
                    "{message}"
                );
            }
        }
    }
}

impl<K: TransactionKind> Drop for MetricsHandler<K> {
    fn drop(&mut self) {
        if !self.close_recorded {
            self.log_backtrace_on_long_read_transaction();
            self.env_metrics.record_closed_transaction(
                self.transaction_mode(),
                TransactionOutcome::Drop,
                self.start.elapsed(),
                None,
                None,
            );
        }
    }
}

impl TableImporter for Tx<RW> {}

impl<K: TransactionKind> DbTx for Tx<K> {
    type Cursor<T: Table> = Cursor<K, T>;
    type DupCursor<T: DupSort> = Cursor<K, T>;

    fn get<T: Table>(&self, key: T::Key) -> Result<Option<<T as Table>::Value>, DatabaseError> {
        self.execute_with_operation_metric::<T, _>(Operation::Get, None, |tx| {
            tx.get(self.get_dbi::<T>()?, key.encode().as_ref())
                .map_err(|e| DatabaseError::Read(e.into()))?
                .map(decode_one::<T>)
                .transpose()
        })
    }

    fn commit(self) -> Result<bool, DatabaseError> {
        self.execute_with_close_transaction_metric(TransactionOutcome::Commit, |this| {
            match this.inner.commit().map_err(|e| DatabaseError::Commit(e.into())) {
                Ok((v, latency)) => (Ok(v), Some(latency)),
                Err(e) => (Err(e), None),
            }
        })
    }

    fn abort(self) {
        self.execute_with_close_transaction_metric(TransactionOutcome::Abort, |this| {
            (drop(this.inner), None)
        })
    }

    // Iterate over read only values in database.
    fn cursor_read<T: Table>(&self) -> Result<Self::Cursor<T>, DatabaseError> {
        self.new_cursor()
    }

    /// Iterate over read only values in database.
    fn cursor_dup_read<T: DupSort>(&self) -> Result<Self::DupCursor<T>, DatabaseError> {
        self.new_cursor()
    }

    /// Returns number of entries in the table using cheap DB stats invocation.
    fn entries<T: Table>(&self) -> Result<usize, DatabaseError> {
        Ok(self
            .inner
            .db_stat_with_dbi(self.get_dbi::<T>()?)
            .map_err(|e| DatabaseError::Stats(e.into()))?
            .entries())
    }

    /// Disables long-lived read transaction safety guarantees, such as backtrace recording and
    /// timeout.
    fn disable_long_read_transaction_safety(&mut self) {
        if let Some(metrics_handler) = self.metrics_handler.as_mut() {
            metrics_handler.record_backtrace = false;
        }

        self.inner.disable_timeout();
    }
}

impl DbTxMut for Tx<RW> {
    type CursorMut<T: Table> = Cursor<RW, T>;
    type DupCursorMut<T: DupSort> = Cursor<RW, T>;

    fn put<T: Table>(&self, key: T::Key, value: T::Value) -> Result<(), DatabaseError> {
        let key = key.encode();
        let value = value.compress();
        self.execute_with_operation_metric::<T, _>(
            Operation::Put,
            Some(value.as_ref().len()),
            |tx| {
                tx.put(self.get_dbi::<T>()?, key.as_ref(), value, WriteFlags::UPSERT).map_err(|e| {
                    DatabaseWriteError {
                        info: e.into(),
                        operation: DatabaseWriteOperation::Put,
                        table_name: T::NAME,
                        key: key.into(),
                    }
                    .into()
                })
            },
        )
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

        self.execute_with_operation_metric::<T, _>(Operation::Delete, None, |tx| {
            tx.del(self.get_dbi::<T>()?, key.encode(), data)
                .map_err(|e| DatabaseError::Delete(e.into()))
        })
    }

    fn clear<T: Table>(&self) -> Result<(), DatabaseError> {
        self.inner.clear_db(self.get_dbi::<T>()?).map_err(|e| DatabaseError::Delete(e.into()))?;

        Ok(())
    }

    fn cursor_write<T: Table>(&self) -> Result<Self::CursorMut<T>, DatabaseError> {
        self.new_cursor()
    }

    fn cursor_dup_write<T: DupSort>(&self) -> Result<Self::DupCursorMut<T>, DatabaseError> {
        self.new_cursor()
    }
}

#[cfg(test)]
mod tests {
    use crate::{mdbx::DatabaseArguments, tables, DatabaseEnv, DatabaseEnvKind};
    use reth_db_api::{database::Database, models::ClientVersion, transaction::DbTx};
    use reth_libmdbx::MaxReadTransactionDuration;
    use reth_storage_errors::db::DatabaseError;
    use std::{sync::atomic::Ordering, thread::sleep, time::Duration};
    use tempfile::tempdir;

    #[test]
    fn long_read_transaction_safety_disabled() {
        const MAX_DURATION: Duration = Duration::from_secs(1);

        let dir = tempdir().unwrap();
        let args = DatabaseArguments::new(ClientVersion::default())
            .with_max_read_transaction_duration(Some(MaxReadTransactionDuration::Set(
                MAX_DURATION,
            )));
        let db = DatabaseEnv::open(dir.path(), DatabaseEnvKind::RW, args).unwrap().with_metrics();

        let mut tx = db.tx().unwrap();
        tx.metrics_handler.as_mut().unwrap().long_transaction_duration = MAX_DURATION;
        tx.disable_long_read_transaction_safety();
        // Give the `TxnManager` some time to time out the transaction.
        sleep(MAX_DURATION + Duration::from_millis(100));

        // Transaction has not timed out.
        assert_eq!(
            tx.get::<tables::Transactions>(0),
            Err(DatabaseError::Open(reth_libmdbx::Error::NotFound.into()))
        );
        // Backtrace is not recorded.
        assert!(!tx.metrics_handler.unwrap().backtrace_recorded.load(Ordering::Relaxed));
    }

    #[test]
    fn long_read_transaction_safety_enabled() {
        const MAX_DURATION: Duration = Duration::from_secs(1);

        let dir = tempdir().unwrap();
        let args = DatabaseArguments::new(ClientVersion::default())
            .with_max_read_transaction_duration(Some(MaxReadTransactionDuration::Set(
                MAX_DURATION,
            )));
        let db = DatabaseEnv::open(dir.path(), DatabaseEnvKind::RW, args).unwrap().with_metrics();

        let mut tx = db.tx().unwrap();
        tx.metrics_handler.as_mut().unwrap().long_transaction_duration = MAX_DURATION;
        // Give the `TxnManager` some time to time out the transaction.
        sleep(MAX_DURATION + Duration::from_millis(100));

        // Transaction has timed out.
        assert_eq!(
            tx.get::<tables::Transactions>(0),
            Err(DatabaseError::Open(reth_libmdbx::Error::ReadTransactionTimeout.into()))
        );
        // Backtrace is recorded.
        assert!(tx.metrics_handler.unwrap().backtrace_recorded.load(Ordering::Relaxed));
    }
}

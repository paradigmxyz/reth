use metrics::{Gauge, Histogram};
use reth_libmdbx::CommitLatency;
use reth_metrics::{metrics::Counter, Metrics};
use std::time::{Duration, Instant};

const LARGE_VALUE_THRESHOLD_BYTES: usize = 4096;

/// Transaction mode for the database, either read-only or read-write.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub(crate) enum TransactionMode {
    /// Read-only transaction mode.
    ReadOnly,
    /// Read-write transaction mode.
    ReadWrite,
}
impl TransactionMode {
    /// Returns the transaction mode as a string.
    pub(crate) const fn as_str(&self) -> &'static str {
        match self {
            TransactionMode::ReadOnly => "read-only",
            TransactionMode::ReadWrite => "read-write",
        }
    }

    /// Returns `true` if the transaction mode is read-only.
    pub(crate) const fn is_read_only(&self) -> bool {
        matches!(self, TransactionMode::ReadOnly)
    }
}

/// Transaction outcome after a database operation - commit, abort, or drop.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub(crate) enum TransactionOutcome {
    /// Successful commit of the transaction.
    Commit,
    /// Aborted transaction.
    Abort,
    /// Dropped transaction.
    Drop,
}

impl TransactionOutcome {
    /// Returns the transaction outcome as a string.
    pub(crate) const fn as_str(&self) -> &'static str {
        match self {
            TransactionOutcome::Commit => "commit",
            TransactionOutcome::Abort => "abort",
            TransactionOutcome::Drop => "drop",
        }
    }
}

/// Types of operations conducted on the database: get, put, delete, and various cursor operations.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub(crate) enum Operation {
    /// Database get operation.
    Get,
    /// Database put operation.
    Put,
    /// Database delete operation.
    Delete,
    /// Database cursor upsert operation.
    CursorUpsert,
    /// Database cursor insert operation.
    CursorInsert,
    /// Database cursor append operation.
    CursorAppend,
    /// Database cursor append duplicates operation.
    CursorAppendDup,
    /// Database cursor delete current operation.
    CursorDeleteCurrent,
    /// Database cursor delete current duplicates operation.
    CursorDeleteCurrentDuplicates,
}

impl Operation {
    /// Returns the operation as a string.
    pub(crate) const fn as_str(&self) -> &'static str {
        match self {
            Operation::Get => "get",
            Operation::Put => "put",
            Operation::Delete => "delete",
            Operation::CursorUpsert => "cursor-upsert",
            Operation::CursorInsert => "cursor-insert",
            Operation::CursorAppend => "cursor-append",
            Operation::CursorAppendDup => "cursor-append-dup",
            Operation::CursorDeleteCurrent => "cursor-delete-current",
            Operation::CursorDeleteCurrentDuplicates => "cursor-delete-current-duplicates",
        }
    }
}

/// Enum defining labels for various aspects used in metrics.
enum Labels {
    /// Label representing a table.
    Table,
    /// Label representing a transaction mode.
    TransactionMode,
    /// Label representing a transaction outcome.
    TransactionOutcome,
    /// Label representing a database operation.
    Operation,
}

impl Labels {
    /// Converts each label variant into its corresponding string representation.
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Labels::Table => "table",
            Labels::TransactionMode => "mode",
            Labels::TransactionOutcome => "outcome",
            Labels::Operation => "operation",
        }
    }
}

#[derive(Metrics, Clone)]
#[metrics(scope = "database.transaction")]
pub(crate) struct TransactionMetrics {
    /// Total number of currently open database transactions
    open_total: Gauge,
    /// The time a database transaction has been open
    open_duration_seconds: Histogram,
    /// The time it took to close a database transaction
    close_duration_seconds: Histogram,
    /// The time it took to prepare a transaction commit
    commit_preparation_duration_seconds: Histogram,
    /// Duration of GC update during transaction commit by wall clock
    commit_gc_wallclock_duration_seconds: Histogram,
    /// The time it took to conduct audit of a transaction commit
    commit_audit_duration_seconds: Histogram,
    /// The time it took to write dirty/modified data pages to a filesystem during transaction
    /// commit
    commit_write_duration_seconds: Histogram,
    /// The time it took to sync written data to the disk/storage during transaction commit
    commit_sync_duration_seconds: Histogram,
    /// The time it took to release resources during transaction commit
    commit_ending_duration_seconds: Histogram,
    /// The total duration of a transaction commit
    commit_whole_duration_seconds: Histogram,
    /// User-mode CPU time spent on GC update during transaction commit
    commit_gc_cputime_duration_seconds: Histogram,
}

impl TransactionMetrics {
    /// Record transaction opening.
    pub(crate) fn record_open(mode: TransactionMode) {
        let metrics = Self::new_with_labels(&[(Labels::TransactionMode.as_str(), mode.as_str())]);
        metrics.open_total.increment(1.0);
    }

    /// Record transaction closing with the duration it was open and the duration it took to close
    /// it.
    pub(crate) fn record_close(
        mode: TransactionMode,
        outcome: TransactionOutcome,
        open_duration: Duration,
        close_duration: Option<Duration>,
        commit_latency: Option<CommitLatency>,
    ) {
        let metrics = Self::new_with_labels(&[(Labels::TransactionMode.as_str(), mode.as_str())]);
        metrics.open_total.decrement(1.0);

        let metrics = Self::new_with_labels(&[
            (Labels::TransactionMode.as_str(), mode.as_str()),
            (Labels::TransactionOutcome.as_str(), outcome.as_str()),
        ]);
        metrics.open_duration_seconds.record(open_duration);

        if let Some(close_duration) = close_duration {
            metrics.close_duration_seconds.record(close_duration)
        }

        if let Some(commit_latency) = commit_latency {
            metrics.commit_preparation_duration_seconds.record(commit_latency.preparation());
            metrics.commit_gc_wallclock_duration_seconds.record(commit_latency.gc_wallclock());
            metrics.commit_audit_duration_seconds.record(commit_latency.audit());
            metrics.commit_write_duration_seconds.record(commit_latency.write());
            metrics.commit_sync_duration_seconds.record(commit_latency.sync());
            metrics.commit_ending_duration_seconds.record(commit_latency.ending());
            metrics.commit_whole_duration_seconds.record(commit_latency.whole());
            metrics.commit_gc_cputime_duration_seconds.record(commit_latency.gc_cputime());
        }
    }
}

#[derive(Metrics, Clone)]
#[metrics(scope = "database.operation")]
pub(crate) struct OperationMetrics {
    /// Total number of database operations made
    calls_total: Counter,
    /// The time it took to execute a database operation (put/upsert/insert/append/append_dup) with
    /// value larger than [LARGE_VALUE_THRESHOLD_BYTES] bytes.
    large_value_duration_seconds: Histogram,
}

impl OperationMetrics {
    /// Record operation metric.
    ///
    /// The duration it took to execute the closure is recorded only if the provided `value_size` is
    /// larger than [LARGE_VALUE_THRESHOLD_BYTES].
    pub(crate) fn record<T>(
        table: &'static str,
        operation: Operation,
        value_size: Option<usize>,
        f: impl FnOnce() -> T,
    ) -> T {
        let metrics = Self::new_with_labels(&[
            (Labels::Table.as_str(), table),
            (Labels::Operation.as_str(), operation.as_str()),
        ]);
        metrics.calls_total.increment(1);

        // Record duration only for large values to prevent the performance hit of clock syscall
        // on small operations
        if value_size.map_or(false, |size| size > LARGE_VALUE_THRESHOLD_BYTES) {
            let start = Instant::now();
            let result = f();
            metrics.large_value_duration_seconds.record(start.elapsed());
            result
        } else {
            f()
        }
    }
}

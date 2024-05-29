use crate::Tables;
use metrics::{Gauge, Histogram};
use reth_libmdbx::CommitLatency;
use reth_metrics::{metrics::Counter, Metrics};
use rustc_hash::{FxHashMap, FxHasher};
use std::{
    collections::HashMap,
    hash::BuildHasherDefault,
    time::{Duration, Instant},
};
use strum::{EnumCount, EnumIter, IntoEnumIterator};

const LARGE_VALUE_THRESHOLD_BYTES: usize = 4096;

/// Caches metric handles for database environment to make sure handles are not re-created
/// on every operation.
///
/// Requires a metric recorder to be registered before creating an instance of this struct.
/// Otherwise, metric recording will no-op.
#[derive(Debug)]
pub struct DatabaseEnvMetrics {
    /// Caches OperationMetrics handles for each table and operation tuple.
    operations: FxHashMap<(Tables, Operation), OperationMetrics>,
    /// Caches TransactionMetrics handles for counters grouped by only transaction mode.
    /// Updated both at tx open and close.
    transactions: FxHashMap<TransactionMode, TransactionMetrics>,
    /// Caches TransactionOutcomeMetrics handles for counters grouped by transaction mode and
    /// outcome. Can only be updated at tx close, as outcome is only known at that point.
    transaction_outcomes:
        FxHashMap<(TransactionMode, TransactionOutcome), TransactionOutcomeMetrics>,
}

impl DatabaseEnvMetrics {
    pub(crate) fn new() -> Self {
        // Pre-populate metric handle maps with all possible combinations of labels
        // to avoid runtime locks on the map when recording metrics.
        Self {
            operations: Self::generate_operation_handles(),
            transactions: Self::generate_transaction_handles(),
            transaction_outcomes: Self::generate_transaction_outcome_handles(),
        }
    }

    /// Generate a map of all possible operation handles for each table and operation tuple.
    /// Used for tracking all operation metrics.
    fn generate_operation_handles() -> FxHashMap<(Tables, Operation), OperationMetrics> {
        let mut operations = FxHashMap::with_capacity_and_hasher(
            Tables::COUNT * Operation::COUNT,
            BuildHasherDefault::<FxHasher>::default(),
        );
        for table in Tables::ALL {
            for operation in Operation::iter() {
                operations.insert(
                    (*table, operation),
                    OperationMetrics::new_with_labels(&[
                        (Labels::Table.as_str(), table.name()),
                        (Labels::Operation.as_str(), operation.as_str()),
                    ]),
                );
            }
        }
        operations
    }

    /// Generate a map of all possible transaction modes to metric handles.
    /// Used for tracking a counter of open transactions.
    fn generate_transaction_handles() -> FxHashMap<TransactionMode, TransactionMetrics> {
        TransactionMode::iter()
            .map(|mode| {
                (
                    mode,
                    TransactionMetrics::new_with_labels(&[(
                        Labels::TransactionMode.as_str(),
                        mode.as_str(),
                    )]),
                )
            })
            .collect()
    }

    /// Generate a map of all possible transaction mode and outcome handles.
    /// Used for tracking various stats for finished transactions (e.g. commit duration).
    fn generate_transaction_outcome_handles(
    ) -> FxHashMap<(TransactionMode, TransactionOutcome), TransactionOutcomeMetrics> {
        let mut transaction_outcomes = HashMap::with_capacity_and_hasher(
            TransactionMode::COUNT * TransactionOutcome::COUNT,
            BuildHasherDefault::<FxHasher>::default(),
        );
        for mode in TransactionMode::iter() {
            for outcome in TransactionOutcome::iter() {
                transaction_outcomes.insert(
                    (mode, outcome),
                    TransactionOutcomeMetrics::new_with_labels(&[
                        (Labels::TransactionMode.as_str(), mode.as_str()),
                        (Labels::TransactionOutcome.as_str(), outcome.as_str()),
                    ]),
                );
            }
        }
        transaction_outcomes
    }

    /// Record a metric for database operation executed in `f`.
    /// Panics if a metric recorder is not found for the given table and operation.
    pub(crate) fn record_operation<R>(
        &self,
        table: Tables,
        operation: Operation,
        value_size: Option<usize>,
        f: impl FnOnce() -> R,
    ) -> R {
        self.operations
            .get(&(table, operation))
            .expect("operation & table metric handle not found")
            .record(value_size, f)
    }

    /// Record metrics for opening a database transaction.
    pub(crate) fn record_opened_transaction(&self, mode: TransactionMode) {
        self.transactions
            .get(&mode)
            .expect("transaction mode metric handle not found")
            .record_open();
    }

    /// Record metrics for closing a database transactions.
    pub(crate) fn record_closed_transaction(
        &self,
        mode: TransactionMode,
        outcome: TransactionOutcome,
        open_duration: Duration,
        close_duration: Option<Duration>,
        commit_latency: Option<CommitLatency>,
    ) {
        self.transactions
            .get(&mode)
            .expect("transaction mode metric handle not found")
            .record_close();

        self.transaction_outcomes
            .get(&(mode, outcome))
            .expect("transaction outcome metric handle not found")
            .record(open_duration, close_duration, commit_latency);
    }
}

/// Transaction mode for the database, either read-only or read-write.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, EnumCount, EnumIter)]
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
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, EnumCount, EnumIter)]
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

    /// Returns `true` if the transaction outcome is a commit.
    pub(crate) const fn is_commit(&self) -> bool {
        matches!(self, TransactionOutcome::Commit)
    }
}

/// Types of operations conducted on the database: get, put, delete, and various cursor operations.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, EnumCount, EnumIter)]
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
}

impl TransactionMetrics {
    pub(crate) fn record_open(&self) {
        self.open_total.increment(1.0);
    }

    pub(crate) fn record_close(&self) {
        self.open_total.decrement(1.0);
    }
}

#[derive(Metrics, Clone)]
#[metrics(scope = "database.transaction")]
pub(crate) struct TransactionOutcomeMetrics {
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

impl TransactionOutcomeMetrics {
    /// Record transaction closing with the duration it was open and the duration it took to close
    /// it.
    pub(crate) fn record(
        &self,
        open_duration: Duration,
        close_duration: Option<Duration>,
        commit_latency: Option<CommitLatency>,
    ) {
        self.open_duration_seconds.record(open_duration);

        if let Some(close_duration) = close_duration {
            self.close_duration_seconds.record(close_duration)
        }

        if let Some(commit_latency) = commit_latency {
            self.commit_preparation_duration_seconds.record(commit_latency.preparation());
            self.commit_gc_wallclock_duration_seconds.record(commit_latency.gc_wallclock());
            self.commit_audit_duration_seconds.record(commit_latency.audit());
            self.commit_write_duration_seconds.record(commit_latency.write());
            self.commit_sync_duration_seconds.record(commit_latency.sync());
            self.commit_ending_duration_seconds.record(commit_latency.ending());
            self.commit_whole_duration_seconds.record(commit_latency.whole());
            self.commit_gc_cputime_duration_seconds.record(commit_latency.gc_cputime());
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
    pub(crate) fn record<R>(&self, value_size: Option<usize>, f: impl FnOnce() -> R) -> R {
        self.calls_total.increment(1);

        // Record duration only for large values to prevent the performance hit of clock syscall
        // on small operations
        if value_size.map_or(false, |size| size > LARGE_VALUE_THRESHOLD_BYTES) {
            let start = Instant::now();
            let result = f();
            self.large_value_duration_seconds.record(start.elapsed());
            result
        } else {
            f()
        }
    }
}

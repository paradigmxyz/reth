use crate::Tables;
use metrics::{Gauge, Histogram};
use reth_metrics::{metrics::Counter, Metrics};
use rustc_hash::FxHashMap;
use std::time::{Duration, Instant};
use strum::{EnumCount, EnumIter, IntoEnumIterator};

const LARGE_VALUE_THRESHOLD_BYTES: usize = 4096;

/// Caches metric handles for database environment to make sure handles are not re-created
/// on every operation.
///
/// Requires a metric recorder to be registered before creating an instance of this struct.
/// Otherwise, metric recording will no-op.
#[derive(Debug)]
pub(crate) struct DatabaseEnvMetrics {
    /// Caches `OperationMetrics` handles for each table and operation tuple.
    operations: FxHashMap<(&'static str, Operation), OperationMetrics>,
    /// Caches `TransactionMetrics` handles for counters grouped by only transaction mode.
    /// Updated both at tx open and close.
    transactions: FxHashMap<TransactionMode, TransactionMetrics>,
    /// Caches `TransactionOutcomeMetrics` handles for counters grouped by transaction mode and
    /// outcome. Can only be updated at tx close, as outcome is only known at that point.
    transaction_outcomes:
        FxHashMap<(TransactionMode, TransactionOutcome), TransactionOutcomeMetrics>,
    /// Caches `EdgeArenaMetrics` handles for each table.
    /// Used for tracking parallel subtransaction arena allocation stats.
    edge_arena: FxHashMap<&'static str, EdgeArenaMetrics>,
}

impl DatabaseEnvMetrics {
    pub(crate) fn new() -> Self {
        // Pre-populate metric handle maps with all possible combinations of labels
        // to avoid runtime locks on the map when recording metrics.
        Self {
            operations: Self::generate_operation_handles(),
            transactions: Self::generate_transaction_handles(),
            transaction_outcomes: Self::generate_transaction_outcome_handles(),
            edge_arena: Self::generate_edge_arena_handles(),
        }
    }

    /// Generate a map of all possible operation handles for each table and operation tuple.
    /// Used for tracking all operation metrics.
    fn generate_operation_handles() -> FxHashMap<(&'static str, Operation), OperationMetrics> {
        let mut operations = FxHashMap::with_capacity_and_hasher(
            Tables::COUNT * Operation::COUNT,
            Default::default(),
        );
        for table in Tables::ALL {
            for operation in Operation::iter() {
                operations.insert(
                    (table.name(), operation),
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
        let mut transaction_outcomes = FxHashMap::with_capacity_and_hasher(
            TransactionMode::COUNT * TransactionOutcome::COUNT,
            Default::default(),
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

    /// Generate a map of all table names to edge arena metric handles.
    /// Used for tracking parallel subtransaction arena allocation stats.
    fn generate_edge_arena_handles() -> FxHashMap<&'static str, EdgeArenaMetrics> {
        Tables::ALL
            .iter()
            .map(|table| {
                (
                    table.name(),
                    EdgeArenaMetrics::new_with_labels(&[(Labels::Table.as_str(), table.name())]),
                )
            })
            .collect()
    }

    /// Record a metric for database operation executed in `f`.
    /// Panics if a metric recorder is not found for the given table and operation.
    pub(crate) fn record_operation<R>(
        &self,
        table: &'static str,
        operation: Operation,
        value_size: Option<usize>,
        f: impl FnOnce() -> R,
    ) -> R {
        if let Some(metrics) = self.operations.get(&(table, operation)) {
            metrics.record(value_size, f)
        } else {
            f()
        }
    }

    /// Record metrics for opening a database transaction.
    pub(crate) fn record_opened_transaction(&self, mode: TransactionMode) {
        self.transactions
            .get(&mode)
            .expect("transaction mode metric handle not found")
            .record_open();
    }

    /// Record metrics for closing a database transactions.
    #[cfg(feature = "mdbx")]
    pub(crate) fn record_closed_transaction(
        &self,
        mode: TransactionMode,
        outcome: TransactionOutcome,
        open_duration: Duration,
        close_duration: Option<Duration>,
        commit_latency: Option<reth_libmdbx::CommitLatency>,
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

    /// Record edge arena stats for a subtransaction.
    ///
    /// The table name is looked up from the provided dbi-to-table mapping.
    #[cfg(feature = "mdbx")]
    pub(crate) fn record_edge_arena_stats(
        &self,
        table: &'static str,
        stats: &reth_libmdbx::SubTransactionStats,
    ) {
        if let Some(metrics) = self.edge_arena.get(table) {
            metrics.record(stats);
        }
    }

    /// Record arena hint estimation stats for a table.
    ///
    /// Tracks whether arena hint estimation is working or always hitting floor/cap.
    #[cfg(feature = "mdbx")]
    pub(crate) fn record_arena_estimation(
        &self,
        table: &'static str,
        stats: &ArenaHintEstimationStats,
    ) {
        if let Some(metrics) = self.edge_arena.get(table) {
            metrics.record_estimation(stats);
        }
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
            Self::ReadOnly => "read-only",
            Self::ReadWrite => "read-write",
        }
    }

    /// Returns `true` if the transaction mode is read-only.
    pub(crate) const fn is_read_only(&self) -> bool {
        matches!(self, Self::ReadOnly)
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
            Self::Commit => "commit",
            Self::Abort => "abort",
            Self::Drop => "drop",
        }
    }

    /// Returns `true` if the transaction outcome is a commit.
    pub(crate) const fn is_commit(&self) -> bool {
        matches!(self, Self::Commit)
    }
}

/// Types of operations conducted on the database: get, put, delete, and various cursor operations.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, EnumCount, EnumIter)]
pub(crate) enum Operation {
    /// Database get operation.
    Get,
    /// Database put upsert operation.
    PutUpsert,
    /// Database put append operation.
    PutAppend,
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
            Self::Get => "get",
            Self::PutUpsert => "put-upsert",
            Self::PutAppend => "put-append",
            Self::Delete => "delete",
            Self::CursorUpsert => "cursor-upsert",
            Self::CursorInsert => "cursor-insert",
            Self::CursorAppend => "cursor-append",
            Self::CursorAppendDup => "cursor-append-dup",
            Self::CursorDeleteCurrent => "cursor-delete-current",
            Self::CursorDeleteCurrentDuplicates => "cursor-delete-current-duplicates",
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
    pub(crate) const fn as_str(&self) -> &'static str {
        match self {
            Self::Table => "table",
            Self::TransactionMode => "mode",
            Self::TransactionOutcome => "outcome",
            Self::Operation => "operation",
        }
    }
}

#[derive(Metrics, Clone)]
#[metrics(scope = "database.transaction")]
pub(crate) struct TransactionMetrics {
    /// Total number of opened database transactions (cumulative)
    opened_total: Counter,
    /// Total number of closed database transactions (cumulative)
    closed_total: Counter,
}

impl TransactionMetrics {
    pub(crate) fn record_open(&self) {
        self.opened_total.increment(1);
    }

    pub(crate) fn record_close(&self) {
        self.closed_total.increment(1);
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
    #[cfg(feature = "mdbx")]
    pub(crate) fn record(
        &self,
        open_duration: Duration,
        close_duration: Option<Duration>,
        commit_latency: Option<reth_libmdbx::CommitLatency>,
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
    /// The time it took to execute a database operation (`put/upsert/insert/append/append_dup`)
    /// with value larger than [`LARGE_VALUE_THRESHOLD_BYTES`] bytes.
    large_value_duration_seconds: Histogram,
}

impl OperationMetrics {
    /// Record operation metric.
    ///
    /// The duration it took to execute the closure is recorded only if the provided `value_size` is
    /// larger than [`LARGE_VALUE_THRESHOLD_BYTES`].
    pub(crate) fn record<R>(&self, value_size: Option<usize>, f: impl FnOnce() -> R) -> R {
        self.calls_total.increment(1);

        // Record duration only for large values to prevent the performance hit of clock syscall
        // on small operations
        if value_size.is_some_and(|size| size > LARGE_VALUE_THRESHOLD_BYTES) {
            let start = Instant::now();
            let result = f();
            self.large_value_duration_seconds.record(start.elapsed());
            result
        } else {
            f()
        }
    }
}

/// Metrics for parallel subtransaction (edge mode) arena allocation.
/// Tracks page allocation efficiency from pre-distributed arenas.
#[derive(Metrics, Clone)]
#[metrics(scope = "database.edge")]
pub(crate) struct EdgeArenaMetrics {
    /// Pages allocated from pre-distributed arena (fast path)
    arena_page_allocations: Counter,
    /// Times fallback to parent was needed (arena refill events)
    arena_refill_events: Counter,
    /// Distribution of refill events per subtxn commit (per-batch granularity)
    arena_refills_per_batch: Histogram,
    /// Pages initially distributed to subtxn
    arena_initial_pages: Counter,
    /// Pages returned to parent on commit (not consumed)
    pages_unused: Counter,
    /// Distribution of unused pages per subtxn commit (detects over-allocation)
    pages_unused_per_batch: Histogram,
    /// Pages acquired from parent during fallback (arena refill)
    arena_refill_pages: Counter,
    /// Configured arena size hint for this table (pages)
    arena_hint: Gauge,
    /// Pages reclaimed from GC (garbage collector / freeDB)
    pages_from_gc: Counter,
    /// Pages allocated from end-of-file (extending the database)
    pages_from_eof: Counter,
    /// Raw calculated estimate before floor was applied
    arena_hint_estimated: Gauge,
    /// Final hint value used after floor
    arena_hint_actual: Gauge,
    /// Times the estimate was below floor and floored value was used
    arena_hint_floored_total: Counter,
    /// Current source of hint: 0=estimated, 1=floored
    arena_hint_source: Gauge,
}

pub(crate) use reth_db_api::transaction::{ArenaHintEstimationStats, ArenaHintSource};

impl EdgeArenaMetrics {
    /// Record stats from a single subtransaction.
    pub(crate) fn record(&self, stats: &reth_libmdbx::SubTransactionStats) {
        println!(
            "[ARENA] page_allocations={} refill_events={} initial_pages={} unused={} refill_pages={} hint={} from_gc={} from_eof={}",
            stats.arena_page_allocations,
            stats.arena_refill_events,
            stats.arena_initial_pages,
            stats.pages_unused,
            stats.arena_refill_pages,
            stats.arena_hint,
            stats.pages_from_gc,
            stats.pages_from_eof
        );
        self.arena_page_allocations.increment(stats.arena_page_allocations as u64);
        self.arena_refill_events.increment(stats.arena_refill_events as u64);
        self.arena_refills_per_batch.record(stats.arena_refill_events as f64);
        self.arena_initial_pages.increment(stats.arena_initial_pages as u64);
        self.pages_unused.increment(stats.pages_unused as u64);
        self.pages_unused_per_batch.record(stats.pages_unused as f64);
        self.arena_refill_pages.increment(stats.arena_refill_pages as u64);
        self.arena_hint.set(stats.arena_hint as f64);
        self.pages_from_gc.increment(stats.pages_from_gc as u64);
        self.pages_from_eof.increment(stats.pages_from_eof as u64);
    }

    /// Record estimation stats for arena hint calculation.
    pub(crate) fn record_estimation(&self, stats: &ArenaHintEstimationStats) {
        self.arena_hint_estimated.set(stats.estimated as f64);
        self.arena_hint_actual.set(stats.actual as f64);
        self.arena_hint_source.set(stats.source as i64 as f64);

        match stats.source {
            ArenaHintSource::Floored => self.arena_hint_floored_total.increment(1),
            ArenaHintSource::Estimated => {}
        }
    }
}

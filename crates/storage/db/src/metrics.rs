use metrics::{Gauge, Histogram};
use reth_metrics::{metrics::Counter, Metrics};
use std::time::Duration;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
#[allow(missing_docs)]
pub(crate) enum TransactionMode {
    ReadOnly,
    ReadWrite,
}

impl TransactionMode {
    pub(crate) const fn as_str(&self) -> &'static str {
        match self {
            TransactionMode::ReadOnly => "read-only",
            TransactionMode::ReadWrite => "read-write",
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
#[allow(missing_docs)]
pub(crate) enum TransactionOutcome {
    Commit,
    Abort,
    Drop,
}

impl TransactionOutcome {
    pub(crate) const fn as_str(&self) -> &'static str {
        match self {
            TransactionOutcome::Commit => "commit",
            TransactionOutcome::Abort => "abort",
            TransactionOutcome::Drop => "drop",
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
#[allow(missing_docs)]
pub(crate) enum Operation {
    Get,
    Put,
    Delete,
    CursorUpsert,
    CursorInsert,
    CursorAppend,
    CursorAppendDup,
    CursorDeleteCurrent,
    CursorDeleteCurrentDuplicates,
}

impl Operation {
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

enum Labels {
    Table,
    TransactionMode,
    TransactionOutcome,
    Operation,
}

impl Labels {
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
}

impl TransactionMetrics {
    pub(crate) fn record_open(mode: TransactionMode) {
        let metrics = Self::new_with_labels(&[(Labels::TransactionMode.as_str(), mode.as_str())]);
        metrics.open_total.increment(1.0);
    }

    pub(crate) fn record_close(
        mode: TransactionMode,
        outcome: TransactionOutcome,
        open_duration: Duration,
        close_duration: Option<Duration>,
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
    }
}

#[derive(Metrics, Clone)]
#[metrics(scope = "database.operation")]
pub(crate) struct OperationMetrics {
    /// Total number of database operations made
    calls_total: Counter,
    /// Database operation duration
    duration_seconds: Histogram,
}

impl OperationMetrics {
    pub(crate) fn record(table: &'static str, operation: Operation, duration: Duration) {
        let metrics = Self::new_with_labels(&[
            (Labels::Table.as_str(), table),
            (Labels::Operation.as_str(), operation.as_str()),
        ]);
        metrics.duration_seconds.record(duration);
        metrics.calls_total.increment(1);
    }
}

use std::{
    fmt::{Display, Formatter},
    time::Instant,
};

mod listener;
mod metrics;

pub use listener::{MetricEvent, MetricEventsSender, MetricsListener};

#[derive(Debug, Clone, Copy)]
struct Transaction {
    mode: TransactionMode,
    begin: Instant,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
#[allow(missing_docs)]
pub enum TransactionMode {
    ReadOnly,
    ReadWrite,
}

impl Display for TransactionMode {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            TransactionMode::ReadOnly => write!(f, "read-only"),
            TransactionMode::ReadWrite => write!(f, "read-write"),
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
#[allow(missing_docs)]
pub enum TransactionOutcome {
    Commit,
    Abort,
}

impl Display for TransactionOutcome {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            TransactionOutcome::Commit => write!(f, "commit"),
            TransactionOutcome::Abort => write!(f, "abort"),
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
#[allow(missing_docs)]
pub enum Operation {
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

impl Display for Operation {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Operation::Get => write!(f, "get"),
            Operation::Put => write!(f, "put"),
            Operation::Delete => write!(f, "delete"),
            Operation::CursorUpsert => write!(f, "cursor-upsert"),
            Operation::CursorInsert => write!(f, "cursor-insert"),
            Operation::CursorAppend => write!(f, "cursor-append"),
            Operation::CursorAppendDup => write!(f, "cursor-append-dup"),
            Operation::CursorDeleteCurrent => write!(f, "cursor-delete-current"),
            Operation::CursorDeleteCurrentDuplicates => {
                write!(f, "cursor-delete-current-duplicates")
            }
        }
    }
}

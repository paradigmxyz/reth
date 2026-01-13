use std::{collections::HashMap, time::Duration};

use itertools::Itertools;
use metrics::{Counter, Histogram};
use reth_db::Tables;
use reth_metrics::Metrics;
use strum::{EnumIter, IntoEnumIterator};

const ROCKSDB_TABLES: &[&str] = &[
    Tables::TransactionHashNumbers.name(),
    Tables::AccountsHistory.name(),
    Tables::StoragesHistory.name(),
];

/// Metrics for the `RocksDB` provider.
#[derive(Debug)]
pub(crate) struct RocksDBMetrics {
    operations: HashMap<(&'static str, RocksDBOperation), RocksDBOperationMetrics>,
}

impl Default for RocksDBMetrics {
    fn default() -> Self {
        let mut operations = ROCKSDB_TABLES
            .iter()
            .copied()
            .cartesian_product(RocksDBOperation::iter())
            .map(|(table, operation)| {
                (
                    (table, operation),
                    RocksDBOperationMetrics::new_with_labels(&[
                        ("table", table),
                        ("operation", operation.as_str()),
                    ]),
                )
            })
            .collect::<HashMap<_, _>>();

        // Add special "Batch" entry for batch write operations
        operations.insert(
            ("Batch", RocksDBOperation::BatchWrite),
            RocksDBOperationMetrics::new_with_labels(&[
                ("table", "Batch"),
                ("operation", RocksDBOperation::BatchWrite.as_str()),
            ]),
        );

        Self { operations }
    }
}

impl RocksDBMetrics {
    /// Records operation metrics with the given operation label and table name.
    pub(crate) fn record_operation(
        &self,
        operation: RocksDBOperation,
        table: &'static str,
        duration: Duration,
    ) {
        let metrics =
            self.operations.get(&(table, operation)).expect("operation metrics should exist");

        metrics.calls_total.increment(1);
        metrics.duration_seconds.record(duration.as_secs_f64());
    }
}

/// `RocksDB` operations that are tracked
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumIter)]
pub(crate) enum RocksDBOperation {
    Get,
    Put,
    Delete,
    BatchWrite,
}

impl RocksDBOperation {
    const fn as_str(&self) -> &'static str {
        match self {
            Self::Get => "get",
            Self::Put => "put",
            Self::Delete => "delete",
            Self::BatchWrite => "batch-write",
        }
    }
}

/// Metrics for a specific `RocksDB` operation on a table
#[derive(Metrics, Clone)]
#[metrics(scope = "rocksdb.provider")]
pub(crate) struct RocksDBOperationMetrics {
    /// Total number of calls
    calls_total: Counter,
    /// Duration of operations
    duration_seconds: Histogram,
}

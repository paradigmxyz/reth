use std::{array, time::Duration};

use metrics::{Counter, Histogram};
use reth_db::Tables;
use reth_metrics::Metrics;

pub(super) const ROCKSDB_TABLES: &[&str] = &[
    Tables::TransactionHashNumbers.name(),
    Tables::StoragesHistory.name(),
    Tables::AccountsHistory.name(),
];
const ROCKSDB_TABLE_OPERATION_COUNT: usize = 3;

/// Metrics for the `RocksDB` provider.
#[derive(Debug)]
pub(crate) struct RocksDBMetrics {
    operations: [[RocksDBOperationMetrics; ROCKSDB_TABLE_OPERATION_COUNT]; ROCKSDB_TABLES.len()],
    batch_write: RocksDBOperationMetrics,
}

impl Default for RocksDBMetrics {
    fn default() -> Self {
        let operations = array::from_fn(|table_index| {
            let table = ROCKSDB_TABLES[table_index];
            array::from_fn(|operation_index| {
                let operation = RocksDBOperation::TABLE_OPERATIONS[operation_index];
                RocksDBOperationMetrics::new_with_labels(&[
                    ("table", table),
                    ("operation", operation.as_str()),
                ])
            })
        });

        let batch_write = RocksDBOperationMetrics::new_with_labels(&[
            ("table", "Batch"),
            ("operation", RocksDBOperation::BatchWrite.as_str()),
        ]);

        Self { operations, batch_write }
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
        let metrics = match operation {
            RocksDBOperation::BatchWrite => {
                debug_assert_eq!(table, "Batch");
                &self.batch_write
            }
            operation => {
                let table_index = ROCKSDB_TABLES
                    .iter()
                    .position(|candidate| *candidate == table)
                    .expect("operation table metrics should exist");
                &self.operations[table_index][operation.table_operation_index()]
            }
        };

        metrics.calls_total.increment(1);
        metrics.duration_seconds.record(duration.as_secs_f64());
    }
}

/// `RocksDB` operations that are tracked
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum RocksDBOperation {
    Get,
    Put,
    Delete,
    BatchWrite,
}

impl RocksDBOperation {
    const TABLE_OPERATIONS: [Self; ROCKSDB_TABLE_OPERATION_COUNT] =
        [Self::Get, Self::Put, Self::Delete];

    fn table_operation_index(self) -> usize {
        match self {
            Self::Get => 0,
            Self::Put => 1,
            Self::Delete => 2,
            Self::BatchWrite => unreachable!("batch writes are stored separately"),
        }
    }

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

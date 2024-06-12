use std::time::Duration;

use itertools::Itertools;
use metrics::{Counter, Gauge, Histogram};
use reth_metrics::Metrics;
use reth_primitives::StaticFileSegment;
use strum::{EnumIter, IntoEnumIterator};

/// Metrics for the static file provider.
#[derive(Debug, Default)]
pub struct StaticFileProviderMetrics {
    segments: StaticFileSegmentMetrics,
    segment_operations: StaticFileProviderOperationMetrics,
}

// impl Default for StaticFileProviderMetrics {
//     fn default() -> Self {
//         Self {
//             segments: StaticFileSegment::iter()
//                 .map(|segment| {
//                     (
//                         segment,
//                         StaticFileSegmentMetrics::new_with_labels(&[("segment",
// segment.as_str())]),                     )
//                 })
//                 .collect(),
//             segment_operations: StaticFileSegment::iter()
//                 .cartesian_product(StaticFileProviderOperation::iter())
//                 .map(|(segment, operation)| {
//                     (
//                         (segment, operation),
//                         StaticFileProviderOperationMetrics::new_with_labels(&[
//                             ("segment", segment.as_str()),
//                             ("operation", operation.as_str()),
//                         ]),
//                     )
//                 })
//                 .collect(),
//         }
//     }
// }

impl StaticFileProviderMetrics {
    pub(crate) fn record_segment(
        &self,
        segment: StaticFileSegment,
        size: u64,
        files: usize,
        entries: usize,
    ) {
        match segment {
            StaticFileSegment::Headers => {
                self.segments.headers_size.set(size as f64);
                self.segments.headers_files.set(files as f64);
                self.segments.headers_entries.set(entries as f64);
            }
            StaticFileSegment::Transactions => {
                self.segments.transactions_size.set(size as f64);
                self.segments.transactions_files.set(files as f64);
                self.segments.transactions_entries.set(entries as f64);
            }
            StaticFileSegment::Receipts => {
                self.segments.receipts_size.set(size as f64);
                self.segments.receipts_files.set(files as f64);
                self.segments.receipts_entries.set(entries as f64);
            }
        }
    }

    pub(crate) fn record_segment_operation(
        &self,
        segment: StaticFileSegment,
        operation: StaticFileProviderOperation,
        duration: Option<Duration>,
    ) {
        match (segment, operation) {
            (StaticFileSegment::Headers, StaticFileProviderOperation::InitCursor) => {
                self.segment_operations.headers_init_cursor_calls_total.increment(1);
                if let Some(duration) = duration {
                    self.segment_operations
                        .headers_init_cursor_write_duration_seconds
                        .record(duration.as_secs_f64());
                }
            }
            (StaticFileSegment::Headers, StaticFileProviderOperation::OpenWriter) => {
                self.segment_operations.headers_open_writer_calls_total.increment(1);
                if let Some(duration) = duration {
                    self.segment_operations
                        .headers_open_writer_write_duration_seconds
                        .record(duration.as_secs_f64());
                }
            }
            (StaticFileSegment::Headers, StaticFileProviderOperation::Append) => {
                self.segment_operations.headers_append_calls_total.increment(1);
                if let Some(duration) = duration {
                    self.segment_operations
                        .headers_append_write_duration_seconds
                        .record(duration.as_secs_f64());
                }
            }
            (StaticFileSegment::Headers, StaticFileProviderOperation::Prune) => {
                self.segment_operations.headers_prune_calls_total.increment(1);
                if let Some(duration) = duration {
                    self.segment_operations
                        .headers_prune_write_duration_seconds
                        .record(duration.as_secs_f64());
                }
            }
            (StaticFileSegment::Headers, StaticFileProviderOperation::IncrementBlock) => {
                self.segment_operations.headers_increment_block_calls_total.increment(1);
                if let Some(duration) = duration {
                    self.segment_operations
                        .headers_increment_block_write_duration_seconds
                        .record(duration.as_secs_f64());
                }
            }
            (StaticFileSegment::Headers, StaticFileProviderOperation::CommitWriter) => {
                self.segment_operations.headers_commit_writer_calls_total.increment(1);
                if let Some(duration) = duration {
                    self.segment_operations
                        .headers_commit_writer_write_duration_seconds
                        .record(duration.as_secs_f64());
                }
            }
            (StaticFileSegment::Transactions, StaticFileProviderOperation::InitCursor) => {
                self.segment_operations.transactions_init_cursor_calls_total.increment(1);
                if let Some(duration) = duration {
                    self.segment_operations
                        .transactions_init_cursor_write_duration_seconds
                        .record(duration.as_secs_f64());
                }
            }
            (StaticFileSegment::Transactions, StaticFileProviderOperation::OpenWriter) => {
                self.segment_operations.transactions_open_writer_calls_total.increment(1);
                if let Some(duration) = duration {
                    self.segment_operations
                        .transactions_open_writer_write_duration_seconds
                        .record(duration.as_secs_f64());
                }
            }
            (StaticFileSegment::Transactions, StaticFileProviderOperation::Append) => {
                self.segment_operations.transactions_append_calls_total.increment(1);
                if let Some(duration) = duration {
                    self.segment_operations
                        .transactions_append_write_duration_seconds
                        .record(duration.as_secs_f64());
                }
            }
            (StaticFileSegment::Transactions, StaticFileProviderOperation::Prune) => {
                self.segment_operations.transactions_prune_calls_total.increment(1);
                if let Some(duration) = duration {
                    self.segment_operations
                        .transactions_prune_write_duration_seconds
                        .record(duration.as_secs_f64());
                }
            }
            (StaticFileSegment::Transactions, StaticFileProviderOperation::IncrementBlock) => {
                self.segment_operations.transactions_increment_block_calls_total.increment(1);
                if let Some(duration) = duration {
                    self.segment_operations
                        .transactions_increment_block_write_duration_seconds
                        .record(duration.as_secs_f64());
                }
            }
            (StaticFileSegment::Transactions, StaticFileProviderOperation::CommitWriter) => {
                self.segment_operations.transactions_commit_writer_calls_total.increment(1);
                if let Some(duration) = duration {
                    self.segment_operations
                        .transactions_commit_writer_write_duration_seconds
                        .record(duration.as_secs_f64());
                }
            }
            (StaticFileSegment::Receipts, StaticFileProviderOperation::InitCursor) => {
                self.segment_operations.receipts_init_cursor_calls_total.increment(1);
                if let Some(duration) = duration {
                    self.segment_operations
                        .receipts_init_cursor_write_duration_seconds
                        .record(duration.as_secs_f64());
                }
            }
            (StaticFileSegment::Receipts, StaticFileProviderOperation::OpenWriter) => {
                self.segment_operations.receipts_open_writer_calls_total.increment(1);
                if let Some(duration) = duration {
                    self.segment_operations
                        .receipts_open_writer_write_duration_seconds
                        .record(duration.as_secs_f64());
                }
            }
            (StaticFileSegment::Receipts, StaticFileProviderOperation::Append) => {
                self.segment_operations.receipts_append_calls_total.increment(1);
                if let Some(duration) = duration {
                    self.segment_operations
                        .receipts_append_write_duration_seconds
                        .record(duration.as_secs_f64());
                }
            }
            (StaticFileSegment::Receipts, StaticFileProviderOperation::Prune) => {
                self.segment_operations.receipts_prune_calls_total.increment(1);
                if let Some(duration) = duration {
                    self.segment_operations
                        .receipts_prune_write_duration_seconds
                        .record(duration.as_secs_f64());
                }
            }
            (StaticFileSegment::Receipts, StaticFileProviderOperation::IncrementBlock) => {
                self.segment_operations.receipts_increment_block_calls_total.increment(1);
                if let Some(duration) = duration {
                    self.segment_operations
                        .receipts_increment_block_write_duration_seconds
                        .record(duration.as_secs_f64());
                }
            }
            (StaticFileSegment::Receipts, StaticFileProviderOperation::CommitWriter) => {
                self.segment_operations.receipts_commit_writer_calls_total.increment(1);
                if let Some(duration) = duration {
                    self.segment_operations
                        .receipts_commit_writer_write_duration_seconds
                        .record(duration.as_secs_f64());
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, EnumIter)]
pub(crate) enum StaticFileProviderOperation {
    InitCursor,
    OpenWriter,
    Append,
    Prune,
    IncrementBlock,
    CommitWriter,
}

impl StaticFileProviderOperation {
    const fn as_str(&self) -> &'static str {
        match self {
            Self::InitCursor => "init-cursor",
            Self::OpenWriter => "open-writer",
            Self::Append => "append",
            Self::Prune => "prune",
            Self::IncrementBlock => "increment-block",
            Self::CommitWriter => "commit-writer",
        }
    }
}

/// Metrics for a specific static file segment.
#[derive(Metrics)]
#[metrics(scope = "static_files.segment")]
pub(crate) struct StaticFileSegmentMetrics {
    /// The size of a header static file segment.
    headers_size: Gauge,
    /// The size of a transaction static file segment.
    transactions_size: Gauge,
    /// The size of a receipt static file segment.
    receipts_size: Gauge,
    /// The number of files for a header static file segment.
    headers_files: Gauge,
    /// The number of files for a transaction static file segment.
    transactions_files: Gauge,
    /// The number of files for a receipt static file segment.
    receipts_files: Gauge,
    /// The number of entries for a receipt static file segment.
    headers_entries: Gauge,
    /// The number of entries for a transaction static file segment.
    transactions_entries: Gauge,
    /// The number of entries for a header static file segment.
    receipts_entries: Gauge,
}

#[derive(Metrics)]
#[metrics(scope = "static_files.jar_provider")]
pub(crate) struct StaticFileProviderOperationMetrics {
    /// Total number of calls to the init cursor operation on headers static file segment.
    headers_init_cursor_calls_total: Counter,
    /// Total number of calls to the open writer operation on headers static file segment.
    headers_open_writer_calls_total: Counter,
    /// Total number of calls to the append operation on headers static file segment.
    headers_append_calls_total: Counter,
    /// Total number of calls to the prune operation on headers static file segment.
    headers_prune_calls_total: Counter,
    /// Total number of calls to the increment block operation on headers static file segment.
    headers_increment_block_calls_total: Counter,
    /// Total number of calls to the commit writer operation on headers static file segment.
    headers_commit_writer_calls_total: Counter,
    /// Total number of calls to the init cursor operation on transactions static file segment.
    transactions_init_cursor_calls_total: Counter,
    /// Total number of calls to the open writer operation on transactions static file segment.
    transactions_open_writer_calls_total: Counter,
    /// Total number of calls to the append operation on transactions static file segment.
    transactions_append_calls_total: Counter,
    /// Total number of calls to the prune operation on transactions static file segment.
    transactions_prune_calls_total: Counter,
    /// Total number of calls to the increment block operation on transactions static file segment.
    transactions_increment_block_calls_total: Counter,
    /// Total number of calls to the commit writer operation on transactions static file segment.
    transactions_commit_writer_calls_total: Counter,
    /// Total number of calls to the init cursor operation on receipts static file segment.
    receipts_init_cursor_calls_total: Counter,
    /// Total number of calls to the open writer operation on receipts static file segment.
    receipts_open_writer_calls_total: Counter,
    /// Total number of calls to the append operation on receipts static file segment.
    receipts_append_calls_total: Counter,
    /// Total number of calls to the prune operation on receipts static file segment.
    receipts_prune_calls_total: Counter,
    /// Total number of calls to the increment block operation on receipts static file segment.
    receipts_increment_block_calls_total: Counter,
    /// Total number of calls to the commit writer operation on receipts static file segment.
    receipts_commit_writer_calls_total: Counter,
    /// The time it took to execute the headers static file jar provider operation that initializes
    /// a cursor.
    headers_init_cursor_write_duration_seconds: Histogram,
    /// The time it took to execute the headers static file jar provider operation that opens a
    /// writer.
    headers_open_writer_write_duration_seconds: Histogram,
    /// The time it took to execute the headers static file jar provider operation that appends
    /// data.
    headers_append_write_duration_seconds: Histogram,
    /// The time it took to execute the headers static file jar provider operation that prunes
    /// data.
    headers_prune_write_duration_seconds: Histogram,
    /// The time it took to execute the headers static file jar provider operation that increments
    /// the block.
    headers_increment_block_write_duration_seconds: Histogram,
    /// The time it took to execute the headers static file jar provider operation that commits
    /// the writer.
    headers_commit_writer_write_duration_seconds: Histogram,
    /// The time it took to execute the transactions static file jar provider operation that
    /// initializes a cursor.
    transactions_init_cursor_write_duration_seconds: Histogram,
    /// The time it took to execute the transactions static file jar provider operation that opens
    /// a writer.
    transactions_open_writer_write_duration_seconds: Histogram,
    /// The time it took to execute the transactions static file jar provider operation that
    /// appends data.
    transactions_append_write_duration_seconds: Histogram,
    /// The time it took to execute the transactions static file jar provider operation that prunes
    /// data.
    transactions_prune_write_duration_seconds: Histogram,
    /// The time it took to execute the transactions static file jar provider operation that
    /// increments the block.
    transactions_increment_block_write_duration_seconds: Histogram,
    /// The time it took to execute the transactions static file jar provider operation that
    /// commits the writer.
    transactions_commit_writer_write_duration_seconds: Histogram,
    /// The time it took to execute the receipts static file jar provider operation that
    /// initializes a cursor.
    receipts_init_cursor_write_duration_seconds: Histogram,
    /// The time it took to execute the receipts static file jar provider operation that opens a
    /// writer.
    receipts_open_writer_write_duration_seconds: Histogram,
    /// The time it took to execute the receipts static file jar provider operation that appends
    /// data.
    receipts_append_write_duration_seconds: Histogram,
    /// The time it took to execute the receipts static file jar provider operation that prunes
    /// data.
    receipts_prune_write_duration_seconds: Histogram,
    /// The time it took to execute the receipts static file jar provider operation that increments
    /// the block.
    receipts_increment_block_write_duration_seconds: Histogram,
    /// The time it took to execute the receipts static file jar provider operation that commits
    /// the writer.
    receipts_commit_writer_write_duration_seconds: Histogram,
}

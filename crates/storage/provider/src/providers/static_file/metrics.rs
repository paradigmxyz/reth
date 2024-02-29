use std::{collections::HashMap, time::Duration};

use itertools::Itertools;
use metrics::{Counter, Histogram};
use reth_metrics::Metrics;
use reth_primitives::StaticFileSegment;
use strum::{EnumIter, IntoEnumIterator};

/// Metrics for the static file provider.
#[derive(Debug)]
pub struct StaticFileProviderMetrics {
    segment_operations: HashMap<
        (StaticFileSegment, StaticFileProviderOperation),
        StaticFileProviderOperationMetrics,
    >,
}

impl Default for StaticFileProviderMetrics {
    fn default() -> Self {
        Self {
            segment_operations: StaticFileSegment::iter()
                .cartesian_product(StaticFileProviderOperation::iter())
                .map(|(segment, operation)| {
                    (
                        (segment, operation),
                        StaticFileProviderOperationMetrics::new_with_labels(&[
                            ("segment", segment.as_str()),
                            ("operation", operation.as_str()),
                        ]),
                    )
                })
                .collect(),
        }
    }
}

impl StaticFileProviderMetrics {
    pub(crate) fn record_segment_operation(
        &self,
        segment: StaticFileSegment,
        operation: StaticFileProviderOperation,
        duration: Option<Duration>,
    ) {
        self.segment_operations
            .get(&(segment, operation))
            .expect("segment operation metrics should exist")
            .calls_total
            .increment(1);

        if let Some(duration) = duration {
            self.segment_operations
                .get(&(segment, operation))
                .expect("segment operation metrics should exist")
                .write_duration_seconds
                .record(duration.as_secs_f64());
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

#[derive(Metrics)]
#[metrics(scope = "static_files.jar_provider")]
pub(crate) struct StaticFileProviderOperationMetrics {
    /// Total number of static file jar provider operations made.
    calls_total: Counter,
    /// The time it took to execute the static file jar provider operation that writes data.
    write_duration_seconds: Histogram,
}

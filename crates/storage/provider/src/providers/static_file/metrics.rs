use std::{collections::HashMap, time::Duration};

use itertools::Itertools;
use metrics::{Counter, Gauge, Histogram};
use reth_metrics::Metrics;
use reth_primitives::StaticFileSegment;
use strum::{EnumIter, IntoEnumIterator};

/// Metrics for the static file provider.
#[derive(Debug)]
pub struct StaticFileProviderMetrics {
    segments: HashMap<StaticFileSegment, StaticFileSegmentMetrics>,
    segment_operations: HashMap<
        (StaticFileSegment, StaticFileProviderOperation),
        StaticFileProviderOperationMetrics,
    >,
}

impl Default for StaticFileProviderMetrics {
    fn default() -> Self {
        Self {
            segments: StaticFileSegment::iter()
                .map(|segment| {
                    (
                        segment,
                        StaticFileSegmentMetrics::new_with_labels(&[("segment", segment.as_str())]),
                    )
                })
                .collect(),
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
    pub(crate) fn record_segment(
        &self,
        segment: StaticFileSegment,
        size: u64,
        files: usize,
        entries: usize,
    ) {
        self.segments.get(&segment).expect("segment metrics should exist").size.set(size as f64);
        self.segments.get(&segment).expect("segment metrics should exist").files.set(files as f64);
        self.segments
            .get(&segment)
            .expect("segment metrics should exist")
            .entries
            .set(entries as f64);
    }

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

/// Metrics for a specific static file segment.
#[derive(Metrics)]
#[metrics(scope = "static_files.segment")]
pub(crate) struct StaticFileSegmentMetrics {
    /// The size of a static file segment
    size: Gauge,
    /// The number of files for a static file segment
    files: Gauge,
    /// The number of entries for a static file segment
    entries: Gauge,
}

#[derive(Metrics)]
#[metrics(scope = "static_files.jar_provider")]
pub(crate) struct StaticFileProviderOperationMetrics {
    /// Total number of static file jar provider operations made.
    calls_total: Counter,
    /// The time it took to execute the static file jar provider operation that writes data.
    write_duration_seconds: Histogram,
}

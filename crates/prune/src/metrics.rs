use reth_metrics::{
    metrics::{Gauge, Histogram},
    Metrics,
};
use reth_primitives::PruneSegment;
use std::collections::HashMap;

#[derive(Metrics)]
#[metrics(scope = "pruner")]
pub(crate) struct Metrics {
    /// Pruning duration
    pub(crate) duration_seconds: Histogram,
    #[metric(skip)]
    prune_segments: HashMap<PruneSegment, PrunerSegmentMetrics>,
}

impl Metrics {
    /// Returns existing or initializes a new instance of [PrunerSegmentMetrics] for the provided
    /// [PruneSegment].
    pub(crate) fn get_prune_segment_metrics(
        &mut self,
        segment: PruneSegment,
    ) -> &mut PrunerSegmentMetrics {
        self.prune_segments.entry(segment).or_insert_with(|| {
            PrunerSegmentMetrics::new_with_labels(&[("segment", segment.to_string())])
        })
    }
}

#[derive(Metrics)]
#[metrics(scope = "pruner.segments")]
pub(crate) struct PrunerSegmentMetrics {
    /// Pruning duration for this segment
    pub(crate) duration_seconds: Histogram,
    /// Highest pruned block per segment
    pub(crate) highest_pruned_block: Gauge,
}

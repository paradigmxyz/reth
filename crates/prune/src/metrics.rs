use reth_metrics::{metrics, metrics::Histogram, Metrics};
use reth_primitives::PrunePart;
use std::collections::HashMap;

#[derive(Metrics)]
#[metrics(scope = "pruner")]
pub(crate) struct Metrics {
    /// Pruning duration
    pub(crate) duration_seconds: Histogram,
    #[metric(skip)]
    prune_parts: HashMap<PrunePart, PrunerPartMetrics>,
}

impl Metrics {
    /// Returns existing or initializes a new instance of [PrunerPartMetrics] for the provided
    /// [PrunePart].
    pub(crate) fn get_prune_part_metrics(
        &mut self,
        prune_part: PrunePart,
    ) -> &mut PrunerPartMetrics {
        self.prune_parts.entry(prune_part).or_insert_with(|| {
            PrunerPartMetrics::new_with_labels(&[("part", prune_part.to_string())])
        })
    }
}

#[derive(Metrics)]
#[metrics(scope = "pruner.parts")]
pub(crate) struct PrunerPartMetrics {
    /// Pruning duration for this part
    pub(crate) duration_seconds: Histogram,
}

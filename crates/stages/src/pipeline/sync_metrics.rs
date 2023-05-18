use crate::StageId;
use metrics::Gauge;
use reth_metrics_derive::Metrics;
use reth_primitives::StageCheckpoint;
use std::collections::HashMap;

#[derive(Metrics)]
#[metrics(scope = "sync")]
pub(crate) struct StageMetrics {
    /// The block number of the last commit for a stage.
    checkpoint: Gauge,
}

#[derive(Default)]
pub(crate) struct Metrics {
    checkpoints: HashMap<StageId, StageMetrics>,
}

impl Metrics {
    pub(crate) fn stage_checkpoint(&mut self, stage_id: StageId, checkpoint: StageCheckpoint) {
        // TODO(alexey): track other metrics from `checkpoint`
        self.checkpoints
            .entry(stage_id)
            .or_insert_with(|| StageMetrics::new_with_labels(&[("stage", stage_id.to_string())]))
            .checkpoint
            .set(checkpoint.block_number as f64);
    }
}

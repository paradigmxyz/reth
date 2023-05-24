use crate::StageId;
use metrics::Gauge;
use reth_metrics_derive::Metrics;
use reth_primitives::{EntitiesCheckpoint, StageCheckpoint, StageUnitCheckpoint};
use std::collections::HashMap;

#[derive(Metrics)]
#[metrics(scope = "sync")]
pub(crate) struct StageMetrics {
    /// The block number of the last commit for a stage.
    checkpoint: Gauge,
    /// The number of processed entities of the last commit for a stage, if applicable.
    entities_processed: Gauge,
    /// The number of total entities of the last commit for a stage, if applicable.
    entities_total: Gauge,
}

#[derive(Default)]
pub(crate) struct Metrics {
    checkpoints: HashMap<StageId, StageMetrics>,
}

impl Metrics {
    pub(crate) fn stage_checkpoint(&mut self, stage_id: StageId, checkpoint: StageCheckpoint) {
        let stage_metrics = self
            .checkpoints
            .entry(stage_id)
            .or_insert_with(|| StageMetrics::new_with_labels(&[("stage", stage_id.to_string())]));

        stage_metrics.checkpoint.set(checkpoint.block_number as f64);

        #[allow(clippy::single_match)]
        match checkpoint.stage_checkpoint {
            Some(StageUnitCheckpoint::Entities(EntitiesCheckpoint { processed, total })) => {
                stage_metrics.entities_processed.set(processed as f64);

                if let Some(total) = total {
                    stage_metrics.entities_total.set(total as f64);
                }
            }
            _ => (),
        }
    }
}

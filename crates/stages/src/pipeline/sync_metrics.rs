use crate::StageId;
use metrics::Gauge;
use reth_metrics_derive::Metrics;
use reth_primitives::{BlockNumber, EntitiesCheckpoint, StageCheckpoint, StageUnitCheckpoint};
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
    stages: HashMap<StageId, StageMetrics>,
}

impl Metrics {
    pub(crate) fn stage_checkpoint(
        &mut self,
        stage_id: StageId,
        checkpoint: StageCheckpoint,
        max_block_number: Option<BlockNumber>,
    ) {
        let stage_metrics = self
            .stages
            .entry(stage_id)
            .or_insert_with(|| StageMetrics::new_with_labels(&[("stage", stage_id.to_string())]));

        stage_metrics.checkpoint.set(checkpoint.block_number as f64);

        let (processed, total) = match checkpoint.stage_checkpoint {
            Some(StageUnitCheckpoint::Entities(EntitiesCheckpoint { processed, total })) => {
                (processed, total)
            }
            _ => (checkpoint.block_number, max_block_number),
        };

        stage_metrics.entities_processed.set(processed as f64);
        if let Some(total) = total {
            stage_metrics.entities_total.set(total as f64);
        }
    }
}

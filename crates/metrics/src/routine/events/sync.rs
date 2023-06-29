use crate::routine::MetricsListener;
use metrics::Gauge;
use reth_metrics_derive::Metrics;
use reth_primitives::{
    stage::{StageCheckpoint, StageId},
    BlockNumber,
};
use std::collections::HashMap;

pub enum SyncMetricEvent {
    StageCheckpoint {
        stage_id: StageId,
        checkpoint: StageCheckpoint,
        max_block_number: Option<BlockNumber>,
    },
}

#[derive(Default)]
pub(crate) struct SyncMetrics {
    stages: HashMap<StageId, StageMetrics>,
}

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

impl MetricsListener {
    pub(crate) fn handle_sync_event(&mut self, event: SyncMetricEvent) {
        match event {
            SyncMetricEvent::StageCheckpoint { stage_id, checkpoint, max_block_number } => {
                let stage_metrics = self.sync_metrics.stages.entry(stage_id).or_insert_with(|| {
                    StageMetrics::new_with_labels(&[("stage", stage_id.to_string())])
                });

                stage_metrics.checkpoint.set(checkpoint.block_number as f64);

                let (processed, total) = match checkpoint.entities() {
                    Some(entities) => (entities.processed, Some(entities.total)),
                    None => (checkpoint.block_number, max_block_number),
                };

                stage_metrics.entities_processed.set(processed as f64);

                if let Some(total) = total {
                    stage_metrics.entities_total.set(total as f64);
                }
            }
        }
    }
}

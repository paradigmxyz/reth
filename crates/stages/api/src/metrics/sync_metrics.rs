use crate::StageId;
use reth_metrics::{metrics::Gauge, Metrics};
use std::collections::HashMap;

#[derive(Debug, Default)]
pub(crate) struct SyncMetrics {
    /// Stage metrics by stage.
    pub(crate) stages: HashMap<StageId, StageMetrics>,
}

impl SyncMetrics {
    /// Returns existing or initializes a new instance of [`StageMetrics`] for the provided
    /// [`StageId`].
    pub(crate) fn get_stage_metrics(&mut self, stage_id: StageId) -> &mut StageMetrics {
        self.stages
            .entry(stage_id)
            .or_insert_with(|| StageMetrics::new_with_labels(&[("stage", stage_id.to_string())]))
    }

    /// Updates all stage checkpoints to the given height efficiently.
    /// This avoids creating multiple individual events when sync height changes.
    pub(crate) fn update_all_stages_height(&mut self, height: u64) {
        for stage_id in StageId::ALL {
            let stage_metrics = self.get_stage_metrics(stage_id);
            stage_metrics.checkpoint.set(height as f64);
            stage_metrics.entities_processed.set(height as f64);
            stage_metrics.entities_total.set(height as f64);
        }
    }
}

#[derive(Metrics)]
#[metrics(scope = "sync")]
pub(crate) struct StageMetrics {
    /// The block number of the last commit for a stage.
    pub(crate) checkpoint: Gauge,
    /// The number of processed entities of the last commit for a stage, if applicable.
    pub(crate) entities_processed: Gauge,
    /// The number of total entities of the last commit for a stage, if applicable.
    pub(crate) entities_total: Gauge,
    /// The number of seconds spent executing the stage and committing the data.
    pub(crate) total_elapsed: Gauge,
}

use crate::StageId;
use reth_metrics::{
    metrics::{Counter, Gauge},
    Metrics,
};
use std::collections::HashMap;

#[derive(Debug, Default)]
pub(crate) struct SyncMetrics {
    pub(crate) stages: HashMap<StageId, StageMetrics>,
    pub(crate) execution_stage: ExecutionStageMetrics,
}

impl SyncMetrics {
    /// Returns existing or initializes a new instance of [`StageMetrics`] for the provided
    /// [`StageId`].
    pub(crate) fn get_stage_metrics(&mut self, stage_id: StageId) -> &mut StageMetrics {
        self.stages
            .entry(stage_id)
            .or_insert_with(|| StageMetrics::new_with_labels(&[("stage", stage_id.to_string())]))
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
}

/// Execution stage metrics.
#[derive(Metrics)]
#[metrics(scope = "sync.execution")]
pub(crate) struct ExecutionStageMetrics {
    /// The total amount of gas processed (in millions)
    pub(crate) mgas_processed_total: Counter,
}

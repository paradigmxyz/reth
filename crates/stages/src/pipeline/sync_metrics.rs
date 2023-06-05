use reth_metrics::{
    metrics::{self, Gauge},
    Metrics,
};
use reth_primitives::{
    stage::{
        AccountHashingCheckpoint, EntitiesCheckpoint, ExecutionCheckpoint, HeadersCheckpoint,
        IndexHistoryCheckpoint, StageCheckpoint, StageId, StageUnitCheckpoint,
        StorageHashingCheckpoint,
    },
    BlockNumber,
};
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
            Some(
                StageUnitCheckpoint::Account(AccountHashingCheckpoint { progress, .. }) |
                StageUnitCheckpoint::Storage(StorageHashingCheckpoint { progress, .. }) |
                StageUnitCheckpoint::Entities(progress @ EntitiesCheckpoint { .. }) |
                StageUnitCheckpoint::Execution(ExecutionCheckpoint { progress, .. }) |
                StageUnitCheckpoint::Headers(HeadersCheckpoint { progress, .. }) |
                StageUnitCheckpoint::IndexHistory(IndexHistoryCheckpoint { progress, .. }),
            ) => (progress.processed, Some(progress.total)),
            None => (checkpoint.block_number, max_block_number),
        };

        stage_metrics.entities_processed.set(processed as f64);

        if let Some(total) = total {
            stage_metrics.entities_total.set(total as f64);
        }
    }
}

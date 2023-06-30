use reth_metrics::{
    metrics::{self, Gauge},
    Metrics,
};
use reth_primitives::stage::StageId;
use std::collections::HashMap;

#[derive(Debug, Default)]
pub(crate) struct SyncMetrics {
    pub(crate) stages: HashMap<StageId, StageMetrics>,
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

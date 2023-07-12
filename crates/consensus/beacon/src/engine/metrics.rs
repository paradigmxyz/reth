use reth_metrics::{
    metrics::{self, Counter, Gauge},
    Metrics,
};

/// Beacon consensus engine metrics.
#[derive(Metrics)]
#[metrics(scope = "consensus.engine.beacon")]
pub(crate) struct EngineMetrics {
    /// The number of times the pipeline was run.
    pub(crate) pipeline_runs: Counter,
    /// The total count of forkchoice updated messages received.
    pub(crate) forkchoice_updated_messages: Counter,
    /// The total count of new payload messages received.
    pub(crate) new_payload_messages: Counter,
    /// The number of times the pruner was run.
    pub(crate) pruner_runs: Counter,
}

/// Metrics for the `EngineSyncController`.
#[derive(Metrics)]
#[metrics(scope = "consensus.engine.beacon")]
pub(crate) struct EngineSyncMetrics {
    /// How many blocks are currently being downloaded.
    pub(crate) active_block_downloads: Gauge,
}

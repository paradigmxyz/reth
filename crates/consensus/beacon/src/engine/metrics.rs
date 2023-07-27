use reth_metrics::{
    metrics::{self, Counter, Gauge, Histogram},
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
    /// Latency for making canonical already canonical block
    pub(crate) make_canonical_already_canonical_latency: Histogram,
    /// Latency for making canonical committed block
    pub(crate) make_canonical_committed_latency: Histogram,
    /// Latency for making canonical returns error
    pub(crate) make_canonical_error_latency: Histogram,
    /// Latency for all making canonical results
    pub(crate) make_canonical_latency: Histogram,
}

/// Metrics for the `EngineSyncController`.
#[derive(Metrics)]
#[metrics(scope = "consensus.engine.beacon")]
pub(crate) struct EngineSyncMetrics {
    /// How many blocks are currently being downloaded.
    pub(crate) active_block_downloads: Gauge,
}

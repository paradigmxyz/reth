use reth_metrics::{
    metrics::{Counter, Gauge},
    Metrics,
};

/// Metrics for the `EngineApi`.
#[derive(Metrics)]
#[metrics(scope = "consensus.engine.beacon")]
pub(crate) struct EngineApiMetrics {
    /// How many executed blocks are currently stored.
    pub(crate) executed_blocks: Gauge,
    /// The number of times the pipeline was run.
    pub(crate) pipeline_runs: Counter,
    /// The total count of forkchoice updated messages received.
    pub(crate) forkchoice_updated_messages: Counter,
    /// The total count of new payload messages received.
    pub(crate) new_payload_messages: Counter,
    // TODO add latency metrics
}

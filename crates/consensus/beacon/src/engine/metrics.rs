use metrics::Counter;
use reth_metrics_derive::Metrics;

/// Beacon consensus engine metrics.
#[derive(Metrics)]
#[metrics(scope = "consensus.engine.beacon")]
pub(crate) struct Metrics {
    /// The number of times the pipeline was run.
    pub(crate) pipeline_runs: Counter,
    /// The total count of forkchoice updated messages received.
    pub(crate) forkchoice_updated_messages: Counter,
    /// The total count of new payload messages received.
    pub(crate) new_payload_messages: Counter,
}

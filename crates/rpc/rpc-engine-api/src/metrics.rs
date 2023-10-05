use metrics::Histogram;
use reth_metrics::Metrics;

/// Beacon consensus engine metrics.
#[derive(Metrics)]
#[metrics(scope = "engine.rpc")]
pub(crate) struct EngineApiMetrics {
    /// Latency for `engine_newPayloadV1`
    pub(crate) new_payload_v1: Histogram,
    /// Latency for `engine_newPayloadV2`
    pub(crate) new_payload_v2: Histogram,
    /// Latency for `engine_newPayloadV3`
    pub(crate) new_payload_v3: Histogram,
    /// Latency for `engine_forkchoiceUpdatedV1`
    pub(crate) fork_choice_updated_v1: Histogram,
    /// Latency for `engine_forkchoiceUpdatedV2`
    pub(crate) fork_choice_updated_v2: Histogram,
    /// Latency for `engine_forkchoiceUpdatedV3`
    pub(crate) fork_choice_updated_v3: Histogram,
    /// Latency for `engine_getPayloadV1`
    pub(crate) get_payload_v1: Histogram,
    /// Latency for `engine_getPayloadV2`
    pub(crate) get_payload_v2: Histogram,
    /// Latency for `engine_getPayloadV3`
    pub(crate) get_payload_v3: Histogram,
    /// Latency for `engine_getPayloadBodiesByRangeV1`
    pub(crate) get_payload_bodies_by_range_v1: Histogram,
    /// Latency for `engine_getPayloadBodiesByHashV1`
    pub(crate) get_payload_bodies_by_hash_v1: Histogram,
    /// Latency for `engine_exchangeTransitionConfigurationV1`
    pub(crate) exchange_transition_configuration: Histogram,
}

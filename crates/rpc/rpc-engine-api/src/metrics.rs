use metrics::{Counter, Histogram};
use reth_metrics::Metrics;

/// All beacon consensus engine metrics
#[derive(Default)]
pub(crate) struct EngineApiMetrics {
    /// Engine API latency metrics
    pub(crate) latency: EngineApiLatencyMetrics,
    /// Blob-related metrics
    pub(crate) blob_metrics: BlobMetrics,
}

/// Beacon consensus engine latency metrics.
#[derive(Metrics)]
#[metrics(scope = "engine.rpc")]
pub(crate) struct EngineApiLatencyMetrics {
    /// Latency for `engine_newPayloadV1`
    pub(crate) new_payload_v1: Histogram,
    /// Latency for `engine_newPayloadV2`
    pub(crate) new_payload_v2: Histogram,
    /// Latency for `engine_newPayloadV3`
    pub(crate) new_payload_v3: Histogram,
    /// Latency for `engine_newPayloadV4`
    pub(crate) new_payload_v4: Histogram,
    /// Latency for `engine_newPayloadV5`
    pub(crate) new_payload_v5: Histogram,
    /// Latency for `engine_forkchoiceUpdatedV1`
    pub(crate) fork_choice_updated_v1: Histogram,
    /// Latency for `engine_forkchoiceUpdatedV2`
    pub(crate) fork_choice_updated_v2: Histogram,
    /// Latency for `engine_forkchoiceUpdatedV3`
    pub(crate) fork_choice_updated_v3: Histogram,
    /// Time diff between `engine_newPayloadV*` and the next FCU
    pub(crate) new_payload_forkchoice_updated_time_diff: Histogram,
    /// Latency for `engine_getPayloadV1`
    pub(crate) get_payload_v1: Histogram,
    /// Latency for `engine_getPayloadV2`
    pub(crate) get_payload_v2: Histogram,
    /// Latency for `engine_getPayloadV3`
    pub(crate) get_payload_v3: Histogram,
    /// Latency for `engine_getPayloadV4`
    pub(crate) get_payload_v4: Histogram,
    /// Latency for `engine_getPayloadV5`
    pub(crate) get_payload_v5: Histogram,
    /// Latency for `engine_getPayloadV6`
    pub(crate) get_payload_v6: Histogram,
    /// Latency for `engine_getPayloadBodiesByRangeV1`
    pub(crate) get_payload_bodies_by_range_v1: Histogram,
    /// Latency for `engine_getPayloadBodiesByHashV1`
    pub(crate) get_payload_bodies_by_hash_v1: Histogram,
    /// Latency for `engine_getBlobsV1`
    pub(crate) get_blobs_v1: Histogram,
    /// Latency for `engine_getBlobsV2`
    pub(crate) get_blobs_v2: Histogram,
}

#[derive(Metrics)]
#[metrics(scope = "engine.rpc.blobs")]
pub(crate) struct BlobMetrics {
    /// Count of blobs successfully retrieved
    pub(crate) blob_count: Counter,
    /// Count of blob misses
    pub(crate) blob_misses: Counter,
    /// Number of blobs requested via getBlobsV2
    pub(crate) get_blobs_requests_blobs_total: Counter,
    /// Number of blobs requested via getBlobsV2 that are present in the blobpool
    pub(crate) get_blobs_requests_blobs_in_blobpool_total: Counter,
    /// Number of times getBlobsV2 responded with “hit”
    pub(crate) get_blobs_requests_success_total: Counter,
    /// Number of times getBlobsV2 responded with “miss”
    pub(crate) get_blobs_requests_failure_total: Counter,
}

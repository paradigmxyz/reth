use std::time::Duration;

use crate::EngineApiError;
use alloy_rpc_types_engine::{ForkchoiceUpdated, PayloadStatus, PayloadStatusEnum};
use metrics::{Counter, Histogram};
use reth_metrics::Metrics;

/// All beacon consensus engine metrics
#[derive(Default)]
pub(crate) struct EngineApiMetrics {
    /// Engine API latency metrics
    pub(crate) latency: EngineApiLatencyMetrics,
    /// Engine API forkchoiceUpdated response type metrics
    pub(crate) fcu_response: ForkchoiceUpdatedResponseMetrics,
    /// Engine API newPayload response type metrics
    pub(crate) new_payload_response: NewPayloadStatusResponseMetrics,
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
    /// Latency for `engine_getPayloadV4`
    pub(crate) get_payload_v4: Histogram,
    /// Latency for `engine_getPayloadBodiesByRangeV1`
    pub(crate) get_payload_bodies_by_range_v1: Histogram,
    /// Latency for `engine_getPayloadBodiesByRangeV2`
    pub(crate) get_payload_bodies_by_range_v2: Histogram,
    /// Latency for `engine_getPayloadBodiesByHashV1`
    pub(crate) get_payload_bodies_by_hash_v1: Histogram,
    /// Latency for `engine_getPayloadBodiesByHashV2`
    pub(crate) get_payload_bodies_by_hash_v2: Histogram,
    /// Latency for `engine_exchangeTransitionConfigurationV1`
    pub(crate) exchange_transition_configuration: Histogram,
}

/// Metrics for engine API forkchoiceUpdated responses.
#[derive(Metrics)]
#[metrics(scope = "engine.rpc")]
pub(crate) struct ForkchoiceUpdatedResponseMetrics {
    /// The total count of forkchoice updated messages received.
    pub(crate) forkchoice_updated_messages: Counter,
    /// The total count of forkchoice updated messages that we responded to with
    /// [`Invalid`](alloy_rpc_types_engine::PayloadStatusEnum#Invalid).
    pub(crate) forkchoice_updated_invalid: Counter,
    /// The total count of forkchoice updated messages that we responded to with
    /// [`Valid`](alloy_rpc_types_engine::PayloadStatusEnum#Valid).
    pub(crate) forkchoice_updated_valid: Counter,
    /// The total count of forkchoice updated messages that we responded to with
    /// [`Syncing`](alloy_rpc_types_engine::PayloadStatusEnum#Syncing).
    pub(crate) forkchoice_updated_syncing: Counter,
    /// The total count of forkchoice updated messages that we responded to with
    /// [`Accepted`](alloy_rpc_types_engine::PayloadStatusEnum#Accepted).
    pub(crate) forkchoice_updated_accepted: Counter,
    /// The total count of forkchoice updated messages that were unsuccessful, i.e. we responded
    /// with an error type that is not a [`PayloadStatusEnum`].
    pub(crate) forkchoice_updated_error: Counter,
}

/// Metrics for engine API newPayload responses.
#[derive(Metrics)]
#[metrics(scope = "engine.rpc")]
pub(crate) struct NewPayloadStatusResponseMetrics {
    /// The total count of new payload messages received.
    pub(crate) new_payload_messages: Counter,
    /// The total count of new payload messages that we responded to with
    /// [Invalid](alloy_rpc_types_engine::PayloadStatusEnum#Invalid).
    pub(crate) new_payload_invalid: Counter,
    /// The total count of new payload messages that we responded to with
    /// [Valid](alloy_rpc_types_engine::PayloadStatusEnum#Valid).
    pub(crate) new_payload_valid: Counter,
    /// The total count of new payload messages that we responded to with
    /// [Syncing](alloy_rpc_types_engine::PayloadStatusEnum#Syncing).
    pub(crate) new_payload_syncing: Counter,
    /// The total count of new payload messages that we responded to with
    /// [Accepted](alloy_rpc_types_engine::PayloadStatusEnum#Accepted).
    pub(crate) new_payload_accepted: Counter,
    /// The total count of new payload messages that were unsuccessful, i.e. we responded with an
    /// error type that is not a [`PayloadStatusEnum`].
    pub(crate) new_payload_error: Counter,
    /// The total gas of valid new payload messages received.
    pub(crate) new_payload_total_gas: Histogram,
    /// The gas per second of valid new payload messages received.
    pub(crate) new_payload_gas_per_second: Histogram,
}

impl NewPayloadStatusResponseMetrics {
    /// Increment the newPayload counter based on the given rpc result
    pub(crate) fn update_response_metrics(
        &self,
        result: &Result<PayloadStatus, EngineApiError>,
        gas_used: u64,
        time: Duration,
    ) {
        match result {
            Ok(status) => match status.status {
                PayloadStatusEnum::Valid => {
                    self.new_payload_valid.increment(1);
                    self.new_payload_total_gas.record(gas_used as f64);
                    self.new_payload_gas_per_second.record(gas_used as f64 / time.as_secs_f64());
                }
                PayloadStatusEnum::Syncing => self.new_payload_syncing.increment(1),
                PayloadStatusEnum::Accepted => self.new_payload_accepted.increment(1),
                PayloadStatusEnum::Invalid { .. } => self.new_payload_invalid.increment(1),
            },
            Err(_) => self.new_payload_error.increment(1),
        }
        self.new_payload_messages.increment(1);
    }
}

impl ForkchoiceUpdatedResponseMetrics {
    /// Increment the forkchoiceUpdated counter based on the given rpc result
    pub(crate) fn update_response_metrics(
        &self,
        result: &Result<ForkchoiceUpdated, EngineApiError>,
    ) {
        match result {
            Ok(status) => match status.payload_status.status {
                PayloadStatusEnum::Valid => self.forkchoice_updated_valid.increment(1),
                PayloadStatusEnum::Syncing => self.forkchoice_updated_syncing.increment(1),
                PayloadStatusEnum::Accepted => self.forkchoice_updated_accepted.increment(1),
                PayloadStatusEnum::Invalid { .. } => self.forkchoice_updated_invalid.increment(1),
            },
            Err(_) => self.forkchoice_updated_error.increment(1),
        }
        self.forkchoice_updated_messages.increment(1);
    }
}

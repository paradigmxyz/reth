//! Optimism supervisor and sequencer metrics

use crate::supervisor::InteropTxValidatorError;
use op_alloy_rpc_types::InvalidInboxEntry;
use reth_metrics::{
    metrics::{Counter, Histogram},
    Metrics,
};
use std::time::Duration;

/// Optimism supervisor metrics
#[derive(Metrics, Clone)]
#[metrics(scope = "optimism_transaction_pool.supervisor")]
pub struct SupervisorMetrics {
    /// How long it takes to query the supervisor in the Optimism transaction pool
    pub(crate) supervisor_query_latency: Histogram,

    /// Counter for the number of times data was skipped
    pub(crate) skipped_data_count: Counter,
    /// Counter for the number of times an unknown chain was encountered
    pub(crate) unknown_chain_count: Counter,
    /// Counter for the number of times conflicting data was encountered
    pub(crate) conflicting_data_count: Counter,
    /// Counter for the number of times ineffective data was encountered
    pub(crate) ineffective_data_count: Counter,
    /// Counter for the number of times data was out of order
    pub(crate) out_of_order_count: Counter,
    /// Counter for the number of times data was awaiting replacement
    pub(crate) awaiting_replacement_count: Counter,
    /// Counter for the number of times data was out of scope
    pub(crate) out_of_scope_count: Counter,
    /// Counter for the number of times there was no parent for the first block
    pub(crate) no_parent_for_first_block_count: Counter,
    /// Counter for the number of times future data was encountered
    pub(crate) future_data_count: Counter,
    /// Counter for the number of times data was missed
    pub(crate) missed_data_count: Counter,
    /// Counter for the number of times data corruption was encountered
    pub(crate) data_corruption_count: Counter,
}

impl SupervisorMetrics {
    /// Records the duration of supervisor queries
    #[inline]
    pub fn record_supervisor_query(&self, duration: Duration) {
        self.supervisor_query_latency.record(duration.as_secs_f64());
    }

    /// Increments the metrics for the given error
    pub fn increment_metrics_for_error(&self, error: &InteropTxValidatorError) {
        if let InteropTxValidatorError::InvalidEntry(inner) = error {
            match inner {
                InvalidInboxEntry::SkippedData => self.skipped_data_count.increment(1),
                InvalidInboxEntry::UnknownChain => self.unknown_chain_count.increment(1),
                InvalidInboxEntry::ConflictingData => self.conflicting_data_count.increment(1),
                InvalidInboxEntry::IneffectiveData => self.ineffective_data_count.increment(1),
                InvalidInboxEntry::OutOfOrder => self.out_of_order_count.increment(1),
                InvalidInboxEntry::AwaitingReplacement => {
                    self.awaiting_replacement_count.increment(1)
                }
                InvalidInboxEntry::OutOfScope => self.out_of_scope_count.increment(1),
                InvalidInboxEntry::NoParentForFirstBlock => {
                    self.no_parent_for_first_block_count.increment(1)
                }
                InvalidInboxEntry::FutureData => self.future_data_count.increment(1),
                InvalidInboxEntry::MissedData => self.missed_data_count.increment(1),
                InvalidInboxEntry::DataCorruption => self.data_corruption_count.increment(1),
                InvalidInboxEntry::UninitializedChainDatabase => {}
            }
        }
    }
}

/// Optimism sequencer metrics
#[derive(Metrics, Clone)]
#[metrics(scope = "optimism_transaction_pool.sequencer")]
pub struct SequencerMetrics {
    /// How long it takes to forward a transaction to the sequencer
    pub(crate) sequencer_forward_latency: Histogram,
}

impl SequencerMetrics {
    /// Records the duration it took to forward a transaction
    #[inline]
    pub fn record_forward_latency(&self, duration: Duration) {
        self.sequencer_forward_latency.record(duration.as_secs_f64());
    }
}

use metrics::Histogram;
use reth_metrics::Metrics;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub(crate) struct DurationsRecorder {
    pub(crate) actions: Vec<(Action, Duration)>,
    latest: Instant,
}

impl Default for DurationsRecorder {
    fn default() -> Self {
        Self { actions: Vec::new(), latest: Instant::now() }
    }
}

impl DurationsRecorder {
    /// Saves the provided duration for future logging and instantly reports as a metric with
    /// `action` label.
    pub(crate) fn record_duration(&mut self, action: Action, duration: Duration) {
        self.actions.push((action, duration));
        Metrics::new_with_labels(&[("action", format!("{action:?}"))]).duration.record(duration);
        self.latest = Instant::now();
    }

    /// Records the duration since last record, saves it for future logging and instantly reports as
    /// a metric with `action` label.
    pub(crate) fn record_relative(&mut self, action: Action) {
        self.record_duration(action, self.latest.elapsed());
    }
}

#[derive(Debug, Copy, Clone)]
pub(crate) enum Action {
    InsertStorageHashing,
    InsertAccountHashing,
    InsertMerkleTree,
    InsertBlock,
    InsertState,
    InsertHashes,
    InsertHistoryIndices,
    UpdatePipelineStages,
    InsertCanonicalHeaders,
    InsertHeaders,
    InsertHeaderNumbers,
    InsertHeaderTD,
    InsertBlockOmmers,
    InsertTxSenders,
    InsertTransactions,
    InsertTxHashNumbers,
    InsertBlockWithdrawals,
    InsertBlockBodyIndices,
    InsertTransactionBlock,

    RecoverSigners,
    GetNextTxNum,
    GetParentTD,
}

#[derive(Metrics)]
#[metrics(scope = "storage.providers.database")]
/// Database provider metrics
struct Metrics {
    /// The time it took to execute an action
    duration: Histogram,
}

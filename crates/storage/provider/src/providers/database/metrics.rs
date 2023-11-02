use metrics::Histogram;
use reth_metrics::Metrics;
use std::time::{Duration, Instant};

#[derive(Debug, Default)]
pub(crate) struct DurationsRecorder {
    pub(crate) actions: Vec<(Action, Duration)>,
}

impl DurationsRecorder {
    /// Records the duration of `f` closure execution, saves for future logging and instantly
    /// reports as a metric with `action` label.
    pub(crate) fn record_closure<T>(&mut self, action: Action, f: impl FnOnce() -> T) -> T {
        let start = Instant::now();
        let result = f();
        self.record_duration(action, start.elapsed());
        result
    }

    /// Saves the provided duration for future logging and instantly reports as a metric with
    /// `action` label.
    pub(crate) fn record_duration(&mut self, action: Action, duration: Duration) {
        self.actions.push((action, duration));
        Metrics::new_with_labels(&[("action", format!("{action:?}"))]).duration.record(duration);
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

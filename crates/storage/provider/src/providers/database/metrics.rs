use metrics::Histogram;
use reth_metrics::Metrics;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub(crate) struct DurationsRecorder {
    start: Instant,
    pub(crate) actions: Vec<(Action, Duration)>,
    latest: Option<Duration>,
}

impl Default for DurationsRecorder {
    fn default() -> Self {
        Self { start: Instant::now(), actions: Vec::new(), latest: None }
    }
}

impl DurationsRecorder {
    /// Saves the provided duration for future logging and instantly reports as a metric with
    /// `action` label.
    pub(crate) fn record_duration(&mut self, action: Action, duration: Duration) {
        self.actions.push((action, duration));
        Metrics::new_with_labels(&[("action", action.as_str())]).duration.record(duration);
        self.latest = Some(self.start.elapsed());
    }

    /// Records the duration since last record, saves it for future logging and instantly reports as
    /// a metric with `action` label.
    pub(crate) fn record_relative(&mut self, action: Action) {
        let elapsed = self.start.elapsed();
        let duration = elapsed - self.latest.unwrap_or_default();

        self.actions.push((action, duration));
        Metrics::new_with_labels(&[("action", action.as_str())]).duration.record(duration);

        self.latest = Some(elapsed);
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

    GetNextTxNum,
    GetParentTD,
}

impl Action {
    fn as_str(&self) -> &'static str {
        match self {
            Action::InsertStorageHashing => "insert storage hashing",
            Action::InsertAccountHashing => "insert account hashing",
            Action::InsertMerkleTree => "insert merkle tree",
            Action::InsertBlock => "insert block",
            Action::InsertState => "insert state",
            Action::InsertHashes => "insert hashes",
            Action::InsertHistoryIndices => "insert history indices",
            Action::UpdatePipelineStages => "update pipeline stages",
            Action::InsertCanonicalHeaders => "insert canonical headers",
            Action::InsertHeaders => "insert headers",
            Action::InsertHeaderNumbers => "insert header numbers",
            Action::InsertHeaderTD => "insert header TD",
            Action::InsertBlockOmmers => "insert block ommers",
            Action::InsertTxSenders => "insert tx senders",
            Action::InsertTransactions => "insert transactions",
            Action::InsertTxHashNumbers => "insert tx hash numbers",
            Action::InsertBlockWithdrawals => "insert block withdrawals",
            Action::InsertBlockBodyIndices => "insert block body indices",
            Action::InsertTransactionBlock => "insert transaction block",
            Action::GetNextTxNum => "get next tx num",
            Action::GetParentTD => "get parent TD",
        }
    }
}

#[derive(Metrics)]
#[metrics(scope = "storage.providers.database")]
/// Database provider metrics
struct Metrics {
    /// The time it took to execute an action
    duration: Histogram,
}

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
    InsertHeaderTerminalDifficulties,
    InsertBlockOmmers,
    InsertTransactionSenders,
    InsertTransactions,
    InsertTransactionHashNumbers,
    InsertBlockWithdrawals,
    InsertBlockRequests,
    InsertBlockBodyIndices,
    InsertTransactionBlocks,

    GetNextTxNum,
    GetParentTD,
}

impl Action {
    const fn as_str(&self) -> &'static str {
        match self {
            Self::InsertStorageHashing => "insert storage hashing",
            Self::InsertAccountHashing => "insert account hashing",
            Self::InsertMerkleTree => "insert merkle tree",
            Self::InsertBlock => "insert block",
            Self::InsertState => "insert state",
            Self::InsertHashes => "insert hashes",
            Self::InsertHistoryIndices => "insert history indices",
            Self::UpdatePipelineStages => "update pipeline stages",
            Self::InsertCanonicalHeaders => "insert canonical headers",
            Self::InsertHeaders => "insert headers",
            Self::InsertHeaderNumbers => "insert header numbers",
            Self::InsertHeaderTerminalDifficulties => "insert header TD",
            Self::InsertBlockOmmers => "insert block ommers",
            Self::InsertTransactionSenders => "insert tx senders",
            Self::InsertTransactions => "insert transactions",
            Self::InsertTransactionHashNumbers => "insert transaction hash numbers",
            Self::InsertBlockWithdrawals => "insert block withdrawals",
            Self::InsertBlockRequests => "insert block withdrawals",
            Self::InsertBlockBodyIndices => "insert block body indices",
            Self::InsertTransactionBlocks => "insert transaction blocks",
            Self::GetNextTxNum => "get next tx num",
            Self::GetParentTD => "get parent TD",
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

use metrics::Histogram;
use reth_metrics::Metrics;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub(crate) struct DurationsRecorder {
    start: Instant,
    current_metrics: DatabaseProviderMetrics,
    pub(crate) actions: Vec<(Action, Duration)>,
    latest: Option<Duration>,
}

impl Default for DurationsRecorder {
    fn default() -> Self {
        Self {
            start: Instant::now(),
            actions: Vec::new(),
            latest: None,
            current_metrics: DatabaseProviderMetrics::default(),
        }
    }
}

impl DurationsRecorder {
    /// Saves the provided duration for future logging and instantly reports as a metric with
    /// `action` label.
    pub(crate) fn record_duration(&mut self, action: Action, duration: Duration) {
        self.actions.push((action, duration));
        self.current_metrics.record_duration(action, duration);
        self.latest = Some(self.start.elapsed());
    }

    /// Records the duration since last record, saves it for future logging and instantly reports as
    /// a metric with `action` label.
    pub(crate) fn record_relative(&mut self, action: Action) {
        let elapsed = self.start.elapsed();
        let duration = elapsed - self.latest.unwrap_or_default();

        self.actions.push((action, duration));
        self.current_metrics.record_duration(action, duration);
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

/// Database provider metrics
#[derive(Metrics)]
#[metrics(scope = "storage.providers.database")]
struct DatabaseProviderMetrics {
    /// Duration of insert storage hashing
    insert_storage_hashing: Histogram,
    /// Duration of insert account hashing
    insert_account_hashing: Histogram,
    /// Duration of insert merkle tree
    insert_merkle_tree: Histogram,
    /// Duration of insert block
    insert_block: Histogram,
    /// Duration of insert state
    insert_state: Histogram,
    /// Duration of insert hashes
    insert_hashes: Histogram,
    /// Duration of insert history indices
    insert_history_indices: Histogram,
    /// Duration of update pipeline stages
    update_pipeline_stages: Histogram,
    /// Duration of insert canonical headers
    insert_canonical_headers: Histogram,
    /// Duration of insert headers
    insert_headers: Histogram,
    /// Duration of insert header numbers
    insert_header_numbers: Histogram,
    /// Duration of insert header TD
    insert_header_td: Histogram,
    /// Duration of insert block ommers
    insert_block_ommers: Histogram,
    /// Duration of insert tx senders
    insert_tx_senders: Histogram,
    /// Duration of insert transactions
    insert_transactions: Histogram,
    /// Duration of insert transaction hash numbers
    insert_tx_hash_numbers: Histogram,
    /// Duration of insert block withdrawals
    insert_block_withdrawals: Histogram,
    /// Duration of insert block requests
    insert_block_requests: Histogram,
    /// Duration of insert block body indices
    insert_block_body_indices: Histogram,
    /// Duration of insert transaction blocks
    insert_tx_blocks: Histogram,
    /// Duration of get next tx num
    get_next_tx_num: Histogram,
    /// Duration of get parent TD
    get_parent_td: Histogram,
}

impl DatabaseProviderMetrics {
    /// Records the duration for the given action.
    pub(crate) fn record_duration(&self, action: Action, duration: Duration) {
        match action {
            Action::InsertStorageHashing => self.insert_storage_hashing.record(duration),
            Action::InsertAccountHashing => self.insert_account_hashing.record(duration),
            Action::InsertMerkleTree => self.insert_merkle_tree.record(duration),
            Action::InsertBlock => self.insert_block.record(duration),
            Action::InsertState => self.insert_state.record(duration),
            Action::InsertHashes => self.insert_hashes.record(duration),
            Action::InsertHistoryIndices => self.insert_history_indices.record(duration),
            Action::UpdatePipelineStages => self.update_pipeline_stages.record(duration),
            Action::InsertCanonicalHeaders => self.insert_canonical_headers.record(duration),
            Action::InsertHeaders => self.insert_headers.record(duration),
            Action::InsertHeaderNumbers => self.insert_header_numbers.record(duration),
            Action::InsertHeaderTerminalDifficulties => self.insert_header_td.record(duration),
            Action::InsertBlockOmmers => self.insert_block_ommers.record(duration),
            Action::InsertTransactionSenders => self.insert_tx_senders.record(duration),
            Action::InsertTransactions => self.insert_transactions.record(duration),
            Action::InsertTransactionHashNumbers => self.insert_tx_hash_numbers.record(duration),
            Action::InsertBlockWithdrawals => self.insert_block_withdrawals.record(duration),
            Action::InsertBlockRequests => self.insert_block_requests.record(duration),
            Action::InsertBlockBodyIndices => self.insert_block_body_indices.record(duration),
            Action::InsertTransactionBlocks => self.insert_tx_blocks.record(duration),
            Action::GetNextTxNum => self.get_next_tx_num.record(duration),
            Action::GetParentTD => self.get_parent_td.record(duration),
        }
    }
}

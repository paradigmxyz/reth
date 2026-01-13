use metrics::Histogram;
use reth_metrics::Metrics;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub(crate) struct DurationsRecorder<'a> {
    start: Instant,
    current_metrics: &'a DatabaseProviderMetrics,
    pub(crate) actions: Vec<(Action, Duration)>,
    latest: Option<Duration>,
}

impl<'a> DurationsRecorder<'a> {
    /// Creates a new durations recorder with the given metrics instance.
    pub(crate) fn new(metrics: &'a DatabaseProviderMetrics) -> Self {
        Self { start: Instant::now(), actions: Vec::new(), latest: None, current_metrics: metrics }
    }
}

impl<'a> DurationsRecorder<'a> {
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
    InsertBlock,
    InsertState,
    InsertHashes,
    InsertHistoryIndices,
    UpdatePipelineStages,
    InsertHeaderNumbers,
    InsertBlockBodyIndices,
    InsertTransactionBlocks,
    GetNextTxNum,
    InsertTransactionSenders,
    InsertTransactionHashNumbers,
    SaveBlocksInsertBlock,
    SaveBlocksWriteState,
    SaveBlocksWriteHashedState,
    SaveBlocksWriteTrieUpdates,
    SaveBlocksUpdateHistoryIndices,
    SaveBlocksUpdatePipelineStages,
}

/// Database provider metrics
#[derive(Metrics)]
#[metrics(scope = "storage.providers.database")]
pub(crate) struct DatabaseProviderMetrics {
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
    /// Duration of insert header numbers
    insert_header_numbers: Histogram,
    /// Duration of insert block body indices
    insert_block_body_indices: Histogram,
    /// Duration of insert transaction blocks
    insert_tx_blocks: Histogram,
    /// Duration of get next tx num
    get_next_tx_num: Histogram,
    /// Duration of insert transaction senders
    insert_transaction_senders: Histogram,
    /// Duration of insert transaction hash numbers
    insert_transaction_hash_numbers: Histogram,
    /// Duration of `insert_block` in `save_blocks`
    save_blocks_insert_block: Histogram,
    /// Duration of `write_state` in `save_blocks`
    save_blocks_write_state: Histogram,
    /// Duration of `write_hashed_state` in `save_blocks`
    save_blocks_write_hashed_state: Histogram,
    /// Duration of `write_trie_updates` in `save_blocks`
    save_blocks_write_trie_updates: Histogram,
    /// Duration of `update_history_indices` in `save_blocks`
    save_blocks_update_history_indices: Histogram,
    /// Duration of `update_pipeline_stages` in `save_blocks`
    save_blocks_update_pipeline_stages: Histogram,
}

impl DatabaseProviderMetrics {
    /// Records the duration for the given action.
    pub(crate) fn record_duration(&self, action: Action, duration: Duration) {
        match action {
            Action::InsertBlock => self.insert_block.record(duration),
            Action::InsertState => self.insert_state.record(duration),
            Action::InsertHashes => self.insert_hashes.record(duration),
            Action::InsertHistoryIndices => self.insert_history_indices.record(duration),
            Action::UpdatePipelineStages => self.update_pipeline_stages.record(duration),
            Action::InsertHeaderNumbers => self.insert_header_numbers.record(duration),
            Action::InsertBlockBodyIndices => self.insert_block_body_indices.record(duration),
            Action::InsertTransactionBlocks => self.insert_tx_blocks.record(duration),
            Action::GetNextTxNum => self.get_next_tx_num.record(duration),
            Action::InsertTransactionSenders => self.insert_transaction_senders.record(duration),
            Action::InsertTransactionHashNumbers => {
                self.insert_transaction_hash_numbers.record(duration)
            }
            Action::SaveBlocksInsertBlock => self.save_blocks_insert_block.record(duration),
            Action::SaveBlocksWriteState => self.save_blocks_write_state.record(duration),
            Action::SaveBlocksWriteHashedState => {
                self.save_blocks_write_hashed_state.record(duration)
            }
            Action::SaveBlocksWriteTrieUpdates => {
                self.save_blocks_write_trie_updates.record(duration)
            }
            Action::SaveBlocksUpdateHistoryIndices => {
                self.save_blocks_update_history_indices.record(duration)
            }
            Action::SaveBlocksUpdatePipelineStages => {
                self.save_blocks_update_pipeline_stages.record(duration)
            }
        }
    }
}

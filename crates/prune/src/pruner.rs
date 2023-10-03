//! Support for pruning.

use crate::{
    segments,
    segments::{PruneInput, Segment},
    Metrics, PrunerError, PrunerEvent,
};
use reth_db::database::Database;
use reth_primitives::{
    listener::EventListeners, BlockNumber, ChainSpec, PruneMode, PruneModes, PruneProgress,
    PruneSegment, PruneSegmentError,
};
use reth_provider::{ProviderFactory, PruneCheckpointReader};
use reth_snapshot::HighestSnapshotsTracker;
use std::{collections::BTreeMap, sync::Arc, time::Instant};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::{debug, trace};

/// Result of [Pruner::run] execution.
pub type PrunerResult = Result<PruneProgress, PrunerError>;

/// The pruner type itself with the result of [Pruner::run]
pub type PrunerWithResult<DB> = (Pruner<DB>, PrunerResult);

/// Pruning routine. Main pruning logic happens in [Pruner::run].
#[derive(Debug)]
pub struct Pruner<DB> {
    metrics: Metrics,
    provider_factory: ProviderFactory<DB>,
    /// Minimum pruning interval measured in blocks. All prune segments are checked and, if needed,
    /// pruned, when the chain advances by the specified number of blocks.
    min_block_interval: usize,
    /// Last pruned block number. Used in conjunction with `min_block_interval` to determine
    /// when the pruning needs to be initiated.
    last_pruned_block_number: Option<BlockNumber>,
    modes: PruneModes,
    /// Maximum total entries to prune (delete from database) per block.
    delete_limit: usize,
    listeners: EventListeners<PrunerEvent>,
    #[allow(dead_code)]
    highest_snapshots_tracker: HighestSnapshotsTracker,
}

impl<DB: Database> Pruner<DB> {
    /// Creates a new [Pruner].
    pub fn new(
        db: DB,
        chain_spec: Arc<ChainSpec>,
        min_block_interval: usize,
        modes: PruneModes,
        delete_limit: usize,
        highest_snapshots_tracker: HighestSnapshotsTracker,
    ) -> Self {
        Self {
            metrics: Metrics::default(),
            provider_factory: ProviderFactory::new(db, chain_spec),
            min_block_interval,
            last_pruned_block_number: None,
            modes,
            delete_limit,
            listeners: Default::default(),
            highest_snapshots_tracker,
        }
    }

    /// Listen for events on the prune.
    pub fn events(&mut self) -> UnboundedReceiverStream<PrunerEvent> {
        self.listeners.new_listener()
    }

    /// Run the pruner
    pub fn run(&mut self, tip_block_number: BlockNumber) -> PrunerResult {
        if tip_block_number == 0 {
            self.last_pruned_block_number = Some(tip_block_number);

            trace!(target: "pruner", %tip_block_number, "Nothing to prune yet");
            return Ok(PruneProgress::Finished)
        }

        trace!(target: "pruner", %tip_block_number, "Pruner started");
        let start = Instant::now();

        let provider = self.provider_factory.provider_rw()?;

        let mut done = true;
        let mut stats = BTreeMap::new();

        // TODO(alexey): prune snapshotted segments of data (headers, transactions)
        // let highest_snapshots = *self.highest_snapshots_tracker.borrow();

        let mut delete_limit = self.delete_limit *
            self.last_pruned_block_number
                .map_or(1, |last_pruned_block_number| tip_block_number - last_pruned_block_number)
                as usize;

        // TODO(alexey): this is cursed, refactor
        #[allow(clippy::type_complexity)]
        let segments: [(
            Box<dyn Segment<DB>>,
            Box<
                dyn Fn(
                    &PruneModes,
                    BlockNumber,
                )
                    -> Result<Option<(BlockNumber, PruneMode)>, PruneSegmentError>,
            >,
        ); 5] = [
            (
                Box::<segments::Receipts>::default(),
                Box::new(PruneModes::prune_target_block_receipts),
            ),
            (
                Box::<segments::TransactionLookup>::default(),
                Box::new(PruneModes::prune_target_block_transaction_lookup),
            ),
            (
                Box::<segments::SenderRecovery>::default(),
                Box::new(PruneModes::prune_target_block_sender_recovery),
            ),
            (
                Box::<segments::AccountHistory>::default(),
                Box::new(PruneModes::prune_target_block_account_history),
            ),
            (
                Box::<segments::StorageHistory>::default(),
                Box::new(PruneModes::prune_target_block_storage_history),
            ),
        ];

        for (segment, get_prune_target_block) in segments {
            if let Some((to_block, prune_mode)) =
                get_prune_target_block(&self.modes, tip_block_number)?
            {
                trace!(
                    target: "pruner",
                    segment = ?segment.segment(),
                    %to_block,
                    ?prune_mode,
                    "Got target block to prune"
                );

                let segment_start = Instant::now();
                let previous_checkpoint = provider.get_prune_checkpoint(segment.segment())?;
                let output = segment
                    .prune(&provider, PruneInput { previous_checkpoint, to_block, delete_limit })?;
                if let Some(checkpoint) = output.checkpoint {
                    segment
                        .save_checkpoint(&provider, checkpoint.as_prune_checkpoint(prune_mode))?;
                }
                self.metrics
                    .get_prune_segment_metrics(segment.segment())
                    .duration_seconds
                    .record(segment_start.elapsed());

                done = done && output.done;
                delete_limit = delete_limit.saturating_sub(output.pruned);
                stats.insert(
                    segment.segment(),
                    (PruneProgress::from_done(output.done), output.pruned),
                );
            } else {
                trace!(target: "pruner", segment = ?segment.segment(), "No target block to prune");
            }
        }

        // TODO(alexey): make it not a special case
        if !self.modes.receipts_log_filter.is_empty() {
            let segment_start = Instant::now();
            let output = segments::ReceiptsByLogs::default().prune_receipts_by_logs(
                &provider,
                &self.modes.receipts_log_filter,
                tip_block_number,
                delete_limit,
            )?;
            self.metrics
                .get_prune_segment_metrics(PruneSegment::ContractLogs)
                .duration_seconds
                .record(segment_start.elapsed());

            done = done && output.done;
            delete_limit = delete_limit.saturating_sub(output.pruned);
            stats.insert(
                PruneSegment::ContractLogs,
                (PruneProgress::from_done(output.done), output.pruned),
            );
        } else {
            trace!(target: "pruner", segment = ?PruneSegment::ContractLogs, "No filter to prune");
        }

        provider.commit()?;
        self.last_pruned_block_number = Some(tip_block_number);

        let elapsed = start.elapsed();
        self.metrics.duration_seconds.record(elapsed);

        trace!(
            target: "pruner",
            %tip_block_number,
            ?elapsed,
            %delete_limit,
            %done,
            ?stats,
            "Pruner finished"
        );

        self.listeners.notify(PrunerEvent::Finished { tip_block_number, elapsed, stats });

        Ok(PruneProgress::from_done(done))
    }

    /// Returns `true` if the pruning is needed at the provided tip block number.
    /// This determined by the check against minimum pruning interval and last pruned block number.
    pub fn is_pruning_needed(&self, tip_block_number: BlockNumber) -> bool {
        if self.last_pruned_block_number.map_or(true, |last_pruned_block_number| {
            // Saturating subtraction is needed for the case when the chain was reverted, meaning
            // current block number might be less than the previously pruned block number. If
            // that's the case, no pruning is needed as outdated data is also reverted.
            tip_block_number.saturating_sub(last_pruned_block_number) >=
                self.min_block_interval as u64
        }) {
            debug!(
                target: "pruner",
                last_pruned_block_number = ?self.last_pruned_block_number,
                %tip_block_number,
                "Minimum pruning interval reached"
            );
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::Pruner;
    use reth_db::test_utils::create_test_rw_db;
    use reth_primitives::{PruneModes, MAINNET};
    use tokio::sync::watch;

    #[test]
    fn is_pruning_needed() {
        let db = create_test_rw_db();
        let mut pruner =
            Pruner::new(db, MAINNET.clone(), 5, PruneModes::none(), 0, watch::channel(None).1);

        // No last pruned block number was set before
        let first_block_number = 1;
        assert!(pruner.is_pruning_needed(first_block_number));
        pruner.last_pruned_block_number = Some(first_block_number);

        // Tip block number delta is >= than min block interval
        let second_block_number = first_block_number + pruner.min_block_interval as u64;
        assert!(pruner.is_pruning_needed(second_block_number));
        pruner.last_pruned_block_number = Some(second_block_number);

        // Tip block number delta is < than min block interval
        let third_block_number = second_block_number;
        assert!(!pruner.is_pruning_needed(third_block_number));
    }
}

//! Support for pruning.

use crate::{
    segments,
    segments::{PruneInput, Segment},
    Metrics, PrunerError, PrunerEvent,
};
use reth_db::database::Database;
use reth_primitives::{BlockNumber, ChainSpec, PruneMode, PruneProgress, PruneSegment};
use reth_provider::{ProviderFactory, PruneCheckpointReader};
use reth_snapshot::HighestSnapshotsTracker;
use reth_tokio_util::EventListeners;
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
    provider_factory: ProviderFactory<DB>,
    segments: Vec<Arc<dyn Segment<DB>>>,
    /// Minimum pruning interval measured in blocks. All prune segments are checked and, if needed,
    /// pruned, when the chain advances by the specified number of blocks.
    min_block_interval: usize,
    /// Previous tip block number when the pruner was run. Even if no data was pruned, this block
    /// number is updated with the tip block number the pruner was called with. It's used in
    /// conjunction with `min_block_interval` to determine when the pruning needs to be initiated.
    previous_tip_block_number: Option<BlockNumber>,
    /// Maximum total entries to prune (delete from database) per block.
    delete_limit: usize,
    /// Maximum number of blocks to be pruned per run, as an additional restriction to
    /// `previous_tip_block_number`.
    prune_max_blocks_per_run: usize,
    #[allow(dead_code)]
    highest_snapshots_tracker: HighestSnapshotsTracker,
    metrics: Metrics,
    listeners: EventListeners<PrunerEvent>,
}

impl<DB: Database> Pruner<DB> {
    /// Creates a new [Pruner].
    pub fn new(
        db: DB,
        chain_spec: Arc<ChainSpec>,
        segments: Vec<Arc<dyn Segment<DB>>>,
        min_block_interval: usize,
        delete_limit: usize,
        prune_max_blocks_per_run: usize,
        highest_snapshots_tracker: HighestSnapshotsTracker,
    ) -> Self {
        Self {
            provider_factory: ProviderFactory::new(db, chain_spec),
            segments,
            min_block_interval,
            previous_tip_block_number: None,
            delete_limit,
            prune_max_blocks_per_run,
            highest_snapshots_tracker,
            metrics: Metrics::default(),
            listeners: Default::default(),
        }
    }

    /// Listen for events on the prune.
    pub fn events(&mut self) -> UnboundedReceiverStream<PrunerEvent> {
        self.listeners.new_listener()
    }

    /// Run the pruner
    pub fn run(&mut self, tip_block_number: BlockNumber) -> PrunerResult {
        if tip_block_number == 0 {
            self.previous_tip_block_number = Some(tip_block_number);

            trace!(target: "pruner", %tip_block_number, "Nothing to prune yet");
            return Ok(PruneProgress::Finished)
        }

        trace!(target: "pruner", %tip_block_number, "Pruner started");
        let start = Instant::now();

        let provider = self.provider_factory.provider_rw()?;

        let mut done = true;
        let mut stats = BTreeMap::new();

        // TODO(alexey): prune snapshotted segments of data (headers, transactions)
        let highest_snapshots = *self.highest_snapshots_tracker.borrow();

        // Multiply `self.delete_limit` (number of rows to delete per block) by number of blocks
        // since last pruner run. `self.previous_tip_block_number` is close to
        // `tip_block_number`, usually within `self.block_interval` blocks, so
        // `delete_limit` will not be too high. If it's too high, we additionally limit it by
        // `self.prune_max_blocks_per_run`.
        //
        // Also see docs for `self.previous_tip_block_number`.
        let blocks_since_last_run =
            (self.previous_tip_block_number.map_or(1, |previous_tip_block_number| {
                // Saturating subtraction is needed for the case when the chain was reverted,
                // meaning current block number might be less than the previous tip
                // block number.
                tip_block_number.saturating_sub(previous_tip_block_number) as usize
            }))
            .min(self.prune_max_blocks_per_run);
        let mut delete_limit = self.delete_limit * blocks_since_last_run;

        for segment in &self.segments {
            if delete_limit == 0 {
                break
            }

            if let Some((to_block, prune_mode)) = segment
                .mode()
                .map(|mode| mode.prune_target_block(tip_block_number, segment.segment()))
                .transpose()?
                .flatten()
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

        if let Some(snapshots) = highest_snapshots {
            if let (Some(to_block), true) = (snapshots.headers, delete_limit > 0) {
                let prune_mode = PruneMode::Before(to_block + 1);
                trace!(
                    target: "pruner",
                    prune_segment = ?PruneSegment::Headers,
                    %to_block,
                    ?prune_mode,
                    "Got target block to prune"
                );

                let segment_start = Instant::now();
                let segment = segments::Headers::new(prune_mode);
                let previous_checkpoint = provider.get_prune_checkpoint(PruneSegment::Headers)?;
                let output = segment
                    .prune(&provider, PruneInput { previous_checkpoint, to_block, delete_limit })?;
                if let Some(checkpoint) = output.checkpoint {
                    segment
                        .save_checkpoint(&provider, checkpoint.as_prune_checkpoint(prune_mode))?;
                }
                self.metrics
                    .get_prune_segment_metrics(PruneSegment::Headers)
                    .duration_seconds
                    .record(segment_start.elapsed());

                done = done && output.done;
                delete_limit = delete_limit.saturating_sub(output.pruned);
                stats.insert(
                    PruneSegment::Headers,
                    (PruneProgress::from_done(output.done), output.pruned),
                );
            }

            if let (Some(to_block), true) = (snapshots.transactions, delete_limit > 0) {
                let prune_mode = PruneMode::Before(to_block + 1);
                trace!(
                    target: "pruner",
                    prune_segment = ?PruneSegment::Transactions,
                    %to_block,
                    ?prune_mode,
                    "Got target block to prune"
                );

                let segment_start = Instant::now();
                let segment = segments::Transactions::new(prune_mode);
                let previous_checkpoint = provider.get_prune_checkpoint(PruneSegment::Headers)?;
                let output = segment
                    .prune(&provider, PruneInput { previous_checkpoint, to_block, delete_limit })?;
                if let Some(checkpoint) = output.checkpoint {
                    segment
                        .save_checkpoint(&provider, checkpoint.as_prune_checkpoint(prune_mode))?;
                }
                self.metrics
                    .get_prune_segment_metrics(PruneSegment::Transactions)
                    .duration_seconds
                    .record(segment_start.elapsed());

                done = done && output.done;
                delete_limit = delete_limit.saturating_sub(output.pruned);
                stats.insert(
                    PruneSegment::Transactions,
                    (PruneProgress::from_done(output.done), output.pruned),
                );
            }
        }

        provider.commit()?;
        self.previous_tip_block_number = Some(tip_block_number);

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
        if self.previous_tip_block_number.map_or(true, |previous_tip_block_number| {
            // Saturating subtraction is needed for the case when the chain was reverted, meaning
            // current block number might be less than the previous tip block number.
            // If that's the case, no pruning is needed as outdated data is also reverted.
            tip_block_number.saturating_sub(previous_tip_block_number) >=
                self.min_block_interval as u64
        }) {
            debug!(
                target: "pruner",
                previous_tip_block_number = ?self.previous_tip_block_number,
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
    use reth_primitives::MAINNET;
    use tokio::sync::watch;

    #[test]
    fn is_pruning_needed() {
        let db = create_test_rw_db();
        let mut pruner = Pruner::new(db, MAINNET.clone(), vec![], 5, 0, 5, watch::channel(None).1);

        // No last pruned block number was set before
        let first_block_number = 1;
        assert!(pruner.is_pruning_needed(first_block_number));
        pruner.previous_tip_block_number = Some(first_block_number);

        // Tip block number delta is >= than min block interval
        let second_block_number = first_block_number + pruner.min_block_interval as u64;
        assert!(pruner.is_pruning_needed(second_block_number));
        pruner.previous_tip_block_number = Some(second_block_number);

        // Tip block number delta is < than min block interval
        let third_block_number = second_block_number;
        assert!(!pruner.is_pruning_needed(third_block_number));
    }
}

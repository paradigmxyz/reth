//! Support for pruning.

use crate::{
    segments,
    segments::{PruneInput, Segment},
    Metrics, PrunerError, PrunerEvent,
};
use reth_db_api::database::Database;
use reth_exex_types::FinishedExExHeight;
use reth_primitives::{BlockNumber, StaticFileSegment};
use reth_provider::{
    DatabaseProviderRW, ProviderFactory, PruneCheckpointReader, StaticFileProviderFactory,
};
use reth_prune_types::{PruneLimiter, PruneMode, PruneProgress, PrunePurpose, PruneSegment};
use reth_tokio_util::{EventSender, EventStream};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};
use tokio::sync::watch;
use tracing::debug;

/// Result of [`Pruner::run`] execution.
pub type PrunerResult = Result<PruneProgress, PrunerError>;

/// The pruner type itself with the result of [`Pruner::run`]
pub type PrunerWithResult<DB> = (Pruner<DB>, PrunerResult);

type PrunerStats = BTreeMap<PruneSegment, (PruneProgress, usize)>;

/// Pruning routine. Main pruning logic happens in [`Pruner::run`].
#[derive(Debug)]
pub struct Pruner<DB> {
    provider_factory: ProviderFactory<DB>,
    segments: Vec<Box<dyn Segment<DB>>>,
    /// Minimum pruning interval measured in blocks. All prune segments are checked and, if needed,
    /// pruned, when the chain advances by the specified number of blocks.
    min_block_interval: usize,
    /// Previous tip block number when the pruner was run. Even if no data was pruned, this block
    /// number is updated with the tip block number the pruner was called with. It's used in
    /// conjunction with `min_block_interval` to determine when the pruning needs to be initiated.
    previous_tip_block_number: Option<BlockNumber>,
    /// Maximum total entries to prune (delete from database) per block.
    delete_limit_per_block: usize,
    /// Maximum number of blocks to be pruned per run, as an additional restriction to
    /// `previous_tip_block_number`.
    prune_max_blocks_per_run: usize,
    /// Maximum time for a one pruner run.
    timeout: Option<Duration>,
    /// The finished height of all `ExEx`'s.
    finished_exex_height: watch::Receiver<FinishedExExHeight>,
    #[doc(hidden)]
    metrics: Metrics,
    event_sender: EventSender<PrunerEvent>,
}

impl<DB: Database> Pruner<DB> {
    /// Creates a new [Pruner].
    pub fn new(
        provider_factory: ProviderFactory<DB>,
        segments: Vec<Box<dyn Segment<DB>>>,
        min_block_interval: usize,
        delete_limit: usize,
        prune_max_blocks_per_run: usize,
        timeout: Option<Duration>,
        finished_exex_height: watch::Receiver<FinishedExExHeight>,
    ) -> Self {
        Self {
            provider_factory,
            segments,
            min_block_interval,
            previous_tip_block_number: None,
            delete_limit_per_block: delete_limit,
            prune_max_blocks_per_run,
            timeout,
            finished_exex_height,
            metrics: Metrics::default(),
            event_sender: Default::default(),
        }
    }

    /// Listen for events on the pruner.
    pub fn events(&self) -> EventStream<PrunerEvent> {
        self.event_sender.new_listener()
    }

    /// Run the pruner
    pub fn run(&mut self, tip_block_number: BlockNumber) -> PrunerResult {
        let Some(tip_block_number) =
            self.adjust_tip_block_number_to_finished_exex_height(tip_block_number)
        else {
            return Ok(PruneProgress::Finished);
        };
        if tip_block_number == 0 {
            self.previous_tip_block_number = Some(tip_block_number);

            debug!(target: "pruner", %tip_block_number, "Nothing to prune yet");
            return Ok(PruneProgress::Finished);
        }

        self.event_sender.notify(PrunerEvent::Started { tip_block_number });

        debug!(target: "pruner", %tip_block_number, "Pruner started");
        let start = Instant::now();

        // Multiply `self.delete_limit` (number of rows to delete per block) by number of blocks
        // since last pruner run. `self.previous_tip_block_number` is close to
        // `tip_block_number`, usually within `self.block_interval` blocks, so
        // `delete_limit` will not be too high. If it's too high, we additionally limit it by
        // `self.prune_max_blocks_per_run`.
        //
        // Also see docs for `self.previous_tip_block_number`.
        let blocks_since_last_run = self
            .previous_tip_block_number
            .map_or(1, |previous_tip_block_number| {
                // Saturating subtraction is needed for the case when the chain was reverted,
                // meaning current block number might be less than the previous tip
                // block number.
                tip_block_number.saturating_sub(previous_tip_block_number) as usize
            })
            .min(self.prune_max_blocks_per_run);

        let mut limiter = PruneLimiter::default()
            .set_deleted_entries_limit(self.delete_limit_per_block * blocks_since_last_run);
        if let Some(timeout) = self.timeout {
            limiter = limiter.set_time_limit(timeout);
        };

        let provider = self.provider_factory.provider_rw()?;
        let (stats, deleted_entries, progress) =
            self.prune_segments(&provider, tip_block_number, &mut limiter)?;
        provider.commit()?;

        self.previous_tip_block_number = Some(tip_block_number);

        let elapsed = start.elapsed();
        self.metrics.duration_seconds.record(elapsed);

        let message = match progress {
            PruneProgress::HasMoreData(_) => "Pruner interrupted and has more data to prune",
            PruneProgress::Finished => "Pruner finished",
        };

        debug!(
            target: "pruner",
            %tip_block_number,
            ?elapsed,
            ?deleted_entries,
            ?limiter,
            ?progress,
            ?stats,
            "{message}",
        );

        self.event_sender.notify(PrunerEvent::Finished { tip_block_number, elapsed, stats });

        Ok(progress)
    }

    /// Prunes the segments that the [Pruner] was initialized with, and the segments that needs to
    /// be pruned according to the highest `static_files`. Segments are parts of the database that
    /// represent one or more tables.
    ///
    /// Returns [`PrunerStats`], total number of entries pruned, and [`PruneProgress`].
    fn prune_segments(
        &mut self,
        provider: &DatabaseProviderRW<DB>,
        tip_block_number: BlockNumber,
        limiter: &mut PruneLimiter,
    ) -> Result<(PrunerStats, usize, PruneProgress), PrunerError> {
        let static_file_segments = self.static_file_segments();
        let segments = static_file_segments
            .iter()
            .map(|segment| (segment, PrunePurpose::StaticFile))
            .chain(self.segments.iter().map(|segment| (segment, PrunePurpose::User)));

        let mut stats = PrunerStats::new();
        let mut pruned = 0;
        let mut progress = PruneProgress::Finished;

        for (segment, purpose) in segments {
            if limiter.is_limit_reached() {
                break;
            }

            if let Some((to_block, prune_mode)) = segment
                .mode()
                .map(|mode| mode.prune_target_block(tip_block_number, segment.segment(), purpose))
                .transpose()?
                .flatten()
            {
                debug!(
                    target: "pruner",
                    segment = ?segment.segment(),
                    ?purpose,
                    %to_block,
                    ?prune_mode,
                    "Segment pruning started"
                );

                let segment_start = Instant::now();
                let previous_checkpoint = provider.get_prune_checkpoint(segment.segment())?;
                let output = segment.prune(
                    provider,
                    PruneInput { previous_checkpoint, to_block, limiter: limiter.clone() },
                )?;
                if let Some(checkpoint) = output.checkpoint {
                    segment
                        .save_checkpoint(provider, checkpoint.as_prune_checkpoint(prune_mode))?;
                }
                self.metrics
                    .get_prune_segment_metrics(segment.segment())
                    .duration_seconds
                    .record(segment_start.elapsed());
                if let Some(highest_pruned_block) =
                    output.checkpoint.and_then(|checkpoint| checkpoint.block_number)
                {
                    self.metrics
                        .get_prune_segment_metrics(segment.segment())
                        .highest_pruned_block
                        .set(highest_pruned_block as f64);
                }

                progress = output.progress;

                debug!(
                    target: "pruner",
                    segment = ?segment.segment(),
                    ?purpose,
                    %to_block,
                    ?prune_mode,
                    %output.pruned,
                    "Segment pruning finished"
                );

                if output.pruned > 0 {
                    limiter.increment_deleted_entries_count_by(output.pruned);
                    pruned += output.pruned;
                    stats.insert(segment.segment(), (output.progress, output.pruned));
                }
            } else {
                debug!(target: "pruner", segment = ?segment.segment(), ?purpose, "Nothing to prune for the segment");
            }
        }

        Ok((stats, pruned, progress))
    }

    /// Returns pre-configured segments that needs to be pruned according to the highest
    /// `static_files` for [`PruneSegment::Transactions`], [`PruneSegment::Headers`] and
    /// [`PruneSegment::Receipts`].
    fn static_file_segments(&self) -> Vec<Box<dyn Segment<DB>>> {
        let mut segments = Vec::<Box<dyn Segment<DB>>>::new();

        let static_file_provider = self.provider_factory.static_file_provider();

        if let Some(to_block) =
            static_file_provider.get_highest_static_file_block(StaticFileSegment::Transactions)
        {
            segments
                .push(Box::new(segments::Transactions::new(PruneMode::before_inclusive(to_block))))
        }

        if let Some(to_block) =
            static_file_provider.get_highest_static_file_block(StaticFileSegment::Headers)
        {
            segments.push(Box::new(segments::Headers::new(PruneMode::before_inclusive(to_block))))
        }

        if let Some(to_block) =
            static_file_provider.get_highest_static_file_block(StaticFileSegment::Receipts)
        {
            segments.push(Box::new(segments::Receipts::new(PruneMode::before_inclusive(to_block))))
        }

        segments
    }

    /// Returns `true` if the pruning is needed at the provided tip block number.
    /// This determined by the check against minimum pruning interval and last pruned block number.
    pub fn is_pruning_needed(&self, tip_block_number: BlockNumber) -> bool {
        let Some(tip_block_number) =
            self.adjust_tip_block_number_to_finished_exex_height(tip_block_number)
        else {
            return false;
        };

        // Saturating subtraction is needed for the case when the chain was reverted, meaning
        // current block number might be less than the previous tip block number.
        // If that's the case, no pruning is needed as outdated data is also reverted.
        if tip_block_number.saturating_sub(self.previous_tip_block_number.unwrap_or_default()) >=
            self.min_block_interval as u64
        {
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

    /// Adjusts the tip block number to the finished `ExEx` height. This is needed to not prune more
    /// data than `ExExs` have processed. Depending on the height:
    /// - [`FinishedExExHeight::NoExExs`] returns the tip block number as is as no adjustment for
    ///   `ExExs` is needed.
    /// - [`FinishedExExHeight::NotReady`] returns `None` as not all `ExExs` have emitted a
    ///   `FinishedHeight` event yet.
    /// - [`FinishedExExHeight::Height`] returns the finished `ExEx` height.
    fn adjust_tip_block_number_to_finished_exex_height(
        &self,
        tip_block_number: BlockNumber,
    ) -> Option<BlockNumber> {
        match *self.finished_exex_height.borrow() {
            FinishedExExHeight::NoExExs => Some(tip_block_number),
            FinishedExExHeight::NotReady => {
                debug!(target: "pruner", %tip_block_number, "Not all ExExs have emitted a `FinishedHeight` event yet, can't prune");
                None
            }
            FinishedExExHeight::Height(finished_exex_height) => {
                debug!(target: "pruner", %tip_block_number, %finished_exex_height, "Adjusting tip block number to the finished ExEx height");
                Some(finished_exex_height)
            }
        }
    }
}

#[cfg(test)]
mod tests {

    use crate::Pruner;
    use reth_db::test_utils::{create_test_rw_db, create_test_static_files_dir};
    use reth_exex_types::FinishedExExHeight;
    use reth_primitives::MAINNET;
    use reth_provider::{providers::StaticFileProvider, ProviderFactory};

    #[test]
    fn is_pruning_needed() {
        let db = create_test_rw_db();
        let (_static_dir, static_dir_path) = create_test_static_files_dir();
        let provider_factory = ProviderFactory::new(
            db,
            MAINNET.clone(),
            StaticFileProvider::read_write(static_dir_path).unwrap(),
        );

        let (finished_exex_height_tx, finished_exex_height_rx) =
            tokio::sync::watch::channel(FinishedExExHeight::NoExExs);

        let mut pruner =
            Pruner::new(provider_factory, vec![], 5, 0, 5, None, finished_exex_height_rx);

        // No last pruned block number was set before
        let first_block_number = 1;
        assert!(!pruner.is_pruning_needed(first_block_number));
        pruner.previous_tip_block_number = Some(first_block_number);

        // Tip block number delta is >= than min block interval
        let second_block_number = first_block_number + pruner.min_block_interval as u64;
        assert!(pruner.is_pruning_needed(second_block_number));
        pruner.previous_tip_block_number = Some(second_block_number);

        // Tip block number delta is < than min block interval
        assert!(!pruner.is_pruning_needed(second_block_number));

        // Tip block number delta is >= than min block interval
        let third_block_number = second_block_number + pruner.min_block_interval as u64;
        assert!(pruner.is_pruning_needed(third_block_number));

        // Not all ExExs have emitted a `FinishedHeight` event yet
        finished_exex_height_tx.send(FinishedExExHeight::NotReady).unwrap();
        assert!(!pruner.is_pruning_needed(third_block_number));

        // Adjust tip block number to the finished ExEx height that doesn't reach the threshold
        finished_exex_height_tx.send(FinishedExExHeight::Height(second_block_number)).unwrap();
        assert!(!pruner.is_pruning_needed(third_block_number));

        // Adjust tip block number to the finished ExEx height that reaches the threshold
        finished_exex_height_tx.send(FinishedExExHeight::Height(third_block_number)).unwrap();
        assert!(pruner.is_pruning_needed(third_block_number));
    }
}

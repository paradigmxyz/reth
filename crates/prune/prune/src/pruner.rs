//! Support for pruning.

use crate::{
    segments::{PruneInput, Segment},
    Metrics, PrunerError, PrunerEvent,
};
use alloy_primitives::BlockNumber;
use reth_db_api::database::Database;
use reth_exex_types::FinishedExExHeight;
use reth_provider::{DatabaseProviderRW, ProviderFactory, PruneCheckpointReader};
use reth_prune_types::{PruneLimiter, PruneProgress, PruneSegment};
use reth_tokio_util::{EventSender, EventStream};
use std::time::{Duration, Instant};
use tokio::sync::watch;
use tracing::debug;

/// Result of [`Pruner::run`] execution.
pub type PrunerResult = Result<PruneProgress, PrunerError>;

/// The pruner type itself with the result of [`Pruner::run`]
pub type PrunerWithResult<S, DB> = (Pruner<S, DB>, PrunerResult);

type PrunerStats = Vec<(PruneSegment, usize, PruneProgress)>;

/// Pruning routine. Main pruning logic happens in [`Pruner::run`].
#[derive(Debug)]
pub struct Pruner<DB, PF> {
    /// Provider factory. If pruner is initialized without it, it will be set to `()`.
    provider_factory: PF,
    segments: Vec<Box<dyn Segment<DB>>>,
    /// Minimum pruning interval measured in blocks. All prune segments are checked and, if needed,
    /// pruned, when the chain advances by the specified number of blocks.
    min_block_interval: usize,
    /// Previous tip block number when the pruner was run. Even if no data was pruned, this block
    /// number is updated with the tip block number the pruner was called with. It's used in
    /// conjunction with `min_block_interval` to determine when the pruning needs to be initiated.
    previous_tip_block_number: Option<BlockNumber>,
    /// Maximum total entries to prune (delete from database) per run.
    delete_limit: usize,
    /// Maximum time for a one pruner run.
    timeout: Option<Duration>,
    /// The finished height of all `ExEx`'s.
    finished_exex_height: watch::Receiver<FinishedExExHeight>,
    #[doc(hidden)]
    metrics: Metrics,
    event_sender: EventSender<PrunerEvent>,
}

impl<DB> Pruner<DB, ()> {
    /// Creates a new [Pruner] without a provider factory.
    pub fn new(
        segments: Vec<Box<dyn Segment<DB>>>,
        min_block_interval: usize,
        delete_limit: usize,
        timeout: Option<Duration>,
        finished_exex_height: watch::Receiver<FinishedExExHeight>,
    ) -> Self {
        Self {
            provider_factory: (),
            segments,
            min_block_interval,
            previous_tip_block_number: None,
            delete_limit,
            timeout,
            finished_exex_height,
            metrics: Metrics::default(),
            event_sender: Default::default(),
        }
    }
}

impl<DB: Database> Pruner<DB, ProviderFactory<DB>> {
    /// Crates a new pruner with the given provider factory.
    pub fn new(
        provider_factory: ProviderFactory<DB>,
        segments: Vec<Box<dyn Segment<DB>>>,
        min_block_interval: usize,
        delete_limit: usize,
        timeout: Option<Duration>,
        finished_exex_height: watch::Receiver<FinishedExExHeight>,
    ) -> Self {
        Self {
            provider_factory,
            segments,
            min_block_interval,
            previous_tip_block_number: None,
            delete_limit,
            timeout,
            finished_exex_height,
            metrics: Metrics::default(),
            event_sender: Default::default(),
        }
    }
}

impl<DB: Database, S> Pruner<DB, S> {
    /// Listen for events on the pruner.
    pub fn events(&self) -> EventStream<PrunerEvent> {
        self.event_sender.new_listener()
    }

    fn run_with_provider(
        &mut self,
        provider: &DatabaseProviderRW<DB>,
        tip_block_number: BlockNumber,
    ) -> PrunerResult {
        let Some(tip_block_number) =
            self.adjust_tip_block_number_to_finished_exex_height(tip_block_number)
        else {
            return Ok(PruneProgress::Finished)
        };
        if tip_block_number == 0 {
            self.previous_tip_block_number = Some(tip_block_number);

            debug!(target: "pruner", %tip_block_number, "Nothing to prune yet");
            return Ok(PruneProgress::Finished)
        }

        self.event_sender.notify(PrunerEvent::Started { tip_block_number });

        debug!(target: "pruner", %tip_block_number, "Pruner started");
        let start = Instant::now();

        let mut limiter = PruneLimiter::default().set_deleted_entries_limit(self.delete_limit);
        if let Some(timeout) = self.timeout {
            limiter = limiter.set_time_limit(timeout);
        };

        let (stats, deleted_entries, progress) =
            self.prune_segments(provider, tip_block_number, &mut limiter)?;

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
        let mut stats = PrunerStats::new();
        let mut pruned = 0;
        let mut progress = PruneProgress::Finished;

        for segment in &self.segments {
            if limiter.is_limit_reached() {
                break
            }

            if let Some((to_block, prune_mode)) = segment
                .mode()
                .map(|mode| {
                    mode.prune_target_block(tip_block_number, segment.segment(), segment.purpose())
                })
                .transpose()?
                .flatten()
            {
                debug!(
                    target: "pruner",
                    segment = ?segment.segment(),
                    purpose = ?segment.purpose(),
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
                    purpose = ?segment.purpose(),
                    %to_block,
                    ?prune_mode,
                    %output.pruned,
                    "Segment pruning finished"
                );

                if output.pruned > 0 {
                    limiter.increment_deleted_entries_count_by(output.pruned);
                    pruned += output.pruned;
                    stats.push((segment.segment(), output.pruned, output.progress));
                }
            } else {
                debug!(target: "pruner", segment = ?segment.segment(), purpose = ?segment.purpose(), "Nothing to prune for the segment");
            }
        }

        Ok((stats, pruned, progress))
    }

    /// Returns `true` if the pruning is needed at the provided tip block number.
    /// This determined by the check against minimum pruning interval and last pruned block number.
    pub fn is_pruning_needed(&self, tip_block_number: BlockNumber) -> bool {
        let Some(tip_block_number) =
            self.adjust_tip_block_number_to_finished_exex_height(tip_block_number)
        else {
            return false
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
    /// - [`FinishedExExHeight::NoExExs`] returns the tip block number as no adjustment for `ExExs`
    ///   is needed.
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

impl<DB: Database> Pruner<DB, ()> {
    /// Run the pruner with the given provider. This will only prune data up to the highest finished
    /// ExEx height, if there are no ExExes.
    ///
    /// Returns a [`PruneProgress`], indicating whether pruning is finished, or there is more data
    /// to prune.
    #[allow(clippy::doc_markdown)]
    pub fn run(
        &mut self,
        provider: &DatabaseProviderRW<DB>,
        tip_block_number: BlockNumber,
    ) -> PrunerResult {
        self.run_with_provider(provider, tip_block_number)
    }
}

impl<DB: Database> Pruner<DB, ProviderFactory<DB>> {
    /// Run the pruner. This will only prune data up to the highest finished ExEx height, if there
    /// are no ExExes.
    ///
    /// Returns a [`PruneProgress`], indicating whether pruning is finished, or there is more data
    /// to prune.
    #[allow(clippy::doc_markdown)]
    pub fn run(&mut self, tip_block_number: BlockNumber) -> PrunerResult {
        let provider = self.provider_factory.provider_rw()?;
        let result = self.run_with_provider(&provider, tip_block_number);
        provider.commit()?;
        result
    }
}

#[cfg(test)]
mod tests {
    use crate::Pruner;
    use reth_exex_types::FinishedExExHeight;
    use reth_provider::{test_utils::create_test_provider_factory, ProviderFactory};

    #[test]
    fn is_pruning_needed() {
        let provider_factory = create_test_provider_factory();

        let (finished_exex_height_tx, finished_exex_height_rx) =
            tokio::sync::watch::channel(FinishedExExHeight::NoExExs);

        let mut pruner = Pruner::<_, ProviderFactory<_>>::new(
            provider_factory,
            vec![],
            5,
            0,
            None,
            finished_exex_height_rx,
        );

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

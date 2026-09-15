//! Persistence state management for background database operations.
//!
//! This module manages the state of background tasks that persist cached data
//! to the database. The persistence system works asynchronously to avoid blocking
//! block execution while ensuring data durability.
//!
//! ## Background Persistence
//!
//! The execution engine maintains an in-memory cache of state changes that need
//! to be persisted to disk. Rather than writing synchronously (which would slow
//! down block processing), persistence happens in background tasks.
//!
//! ## Persistence Actions
//!
//! - **Saving Blocks**: Persist newly executed blocks and their state changes
//! - **Removing Blocks**: Remove invalid blocks during chain reorganizations
//!
//! ## Coordination
//!
//! The [`PersistenceState`] tracks ongoing persistence operations and coordinates
//! between the main execution thread and background persistence workers.

use crate::persistence::PersistenceResult;
use alloy_eips::BlockNumHash;
use alloy_primitives::B256;
use crossbeam_channel::Receiver as CrossbeamReceiver;
use reth_engine_primitives::PersistencePacingFeedback;
use reth_primitives_traits::FastInstant as Instant;
use std::{collections::VecDeque, time::Duration};
use tracing::trace;

/// The state of the persistence task.
#[derive(Debug)]
pub struct PersistenceState {
    /// Hash and number of the highest block whose non-state/trie outputs are persisted.
    ///
    /// This tracks the highest canonical block with durable block/static-file/plain-state data.
    pub(crate) last_persisted_block: BlockNumHash,
    /// Hash and number of the highest block whose state/trie outputs were processed for
    /// persistence.
    pub(crate) last_state_trie_persisted_block: BlockNumHash,
    /// Receiver end of channel where the result of the persistence task will be
    /// sent when done. A None value means there's no persistence task in progress.
    pub(crate) rx:
        Option<(CrossbeamReceiver<PersistenceResult>, Instant, CurrentPersistenceAction)>,
}

impl PersistenceState {
    /// Determines if there is a persistence task in progress by checking if the
    /// receiver is set.
    pub(crate) const fn in_progress(&self) -> bool {
        self.rx.is_some()
    }

    /// Sets the state for a block removal operation.
    pub(crate) fn start_remove(
        &mut self,
        new_tip_num: u64,
        rx: CrossbeamReceiver<PersistenceResult>,
    ) {
        self.rx =
            Some((rx, Instant::now(), CurrentPersistenceAction::RemovingBlocks { new_tip_num }));
    }

    /// Sets the state for a block save operation.
    pub(crate) fn start_save(
        &mut self,
        highest: BlockNumHash,
        rx: CrossbeamReceiver<PersistenceResult>,
    ) {
        self.rx = Some((rx, Instant::now(), CurrentPersistenceAction::SavingBlocks { highest }));
    }

    /// Returns the current persistence action. If there is no persistence task in progress, then
    /// this returns `None`.
    #[cfg(test)]
    pub(crate) fn current_action(&self) -> Option<&CurrentPersistenceAction> {
        self.rx.as_ref().map(|rx| &rx.2)
    }

    /// Sets state for a finished persistence task.
    pub(crate) fn finish(
        &mut self,
        last_persisted_block: BlockNumHash,
        last_state_trie_persisted_block: BlockNumHash,
    ) {
        trace!(
            target: "engine::tree",
            last_persisted_block = %last_persisted_block.number,
            last_state_trie_persisted_block = %last_state_trie_persisted_block.number,
            "updating persistence state"
        );
        self.rx = None;
        self.last_persisted_block = last_persisted_block;
        self.last_state_trie_persisted_block = last_state_trie_persisted_block;
    }
}

/// The currently running persistence action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CurrentPersistenceAction {
    /// The persistence task is saving blocks.
    SavingBlocks {
        /// The highest block being saved.
        highest: BlockNumHash,
    },
    /// The persistence task is removing blocks.
    RemovingBlocks {
        /// The tip, above which we are removing blocks.
        new_tip_num: u64,
    },
}

/// Paces validation using the last three successful saves that released blocks from memory.
#[derive(Debug, Default)]
pub(crate) struct PersistencePacing {
    /// Original local admission feedback, retained across notification/acknowledgment races.
    local_feedback: VecDeque<(B256, PersistencePacingFeedback)>,
    /// Last new block's feedback; changed only when a completion is recorded.
    pub(crate) last_feedback: Option<PersistencePacingFeedback>,
    /// Monotonic completion count distinguishes fresh validation from duplicate requests.
    pub(crate) completions: u64,
    /// Oldest sample first; each sample is save duration per fully persisted block.
    samples: VecDeque<Duration>,
    /// Completion before any pacing sleep, including locally built blocks inserted directly.
    pub(super) last_validation_completed_at: Option<Instant>,
    /// Cumulative sleep at the previous completion, before that block's tail sleep.
    wait_at_last_completion: Duration,
    /// Cumulative sleep, allowing an RPC to account for all blocks it connects.
    pub(crate) total_wait: Duration,
}

impl PersistencePacing {
    const WINDOW: usize = 3;
    const ALPHA: f64 = 2.0 / (Self::WINDOW as f64 + 1.0);

    /// Record every completion, including those below the backpressure threshold. Time between
    /// completions includes payload building and idle time, but excludes the previous block's
    /// pacing sleep: that sleep belongs to the block that incurred it.
    pub(crate) fn on_validation_completed(&mut self, now: Instant, pacing: bool) -> Duration {
        self.completions += 1;
        self.last_feedback = Some(PersistencePacingFeedback {
            adaptive_wait: Duration::ZERO,
            persistence_per_block: pacing.then(|| self.estimate()).flatten(),
        });
        let previous = self.last_validation_completed_at.replace(now);
        let previous_wait = self.total_wait - self.wait_at_last_completion;
        self.wait_at_last_completion = self.total_wait;
        match previous {
            Some(previous) if pacing => {
                self.delay(now.duration_since(previous).saturating_sub(previous_wait))
            }
            _ => Duration::ZERO,
        }
    }

    pub(crate) fn record(&mut self, duration: Duration, blocks: u64) {
        if blocks == 0 {
            return;
        }
        if self.samples.len() == Self::WINDOW {
            self.samples.pop_front();
        }
        self.samples.push_back(duration.div_f64(blocks as f64));
    }

    /// Recompute the EMA over the bounded window so older saves no longer affect pacing.
    pub(crate) fn delay(&self, completion_interval: Duration) -> Duration {
        PersistencePacingFeedback { persistence_per_block: self.estimate(), ..Default::default() }
            .remaining_wait(completion_interval)
    }

    fn estimate(&self) -> Option<Duration> {
        if self.samples.len() < 2 {
            return None;
        }
        let mut samples = self.samples.iter().map(Duration::as_secs_f64);
        let first = samples.next().expect("at least two samples");
        let ema = samples
            .fold(first, |ema, sample| (1.0 - Self::ALPHA).mul_add(ema, Self::ALPHA * sample));
        Some(Duration::from_secs_f64(ema))
    }

    pub(crate) fn record_local_feedback(&mut self, hash: B256) {
        if let Some(feedback) = self.last_feedback {
            if self.local_feedback.len() == 64 {
                self.local_feedback.pop_front();
            }
            self.local_feedback.push_back((hash, feedback));
        }
    }

    pub(crate) fn local_feedback(&self, hash: B256) -> Option<PersistencePacingFeedback> {
        self.local_feedback.iter().rev().find(|(key, _)| *key == hash).map(|(_, value)| *value)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn feedback_is_bounded_and_reports_inactive_completions() {
        let mut pacing = super::PersistencePacing::default();
        let now = Instant::now();
        pacing.record(Duration::from_millis(200), 2);
        pacing.on_validation_completed(now, true);
        assert_eq!(pacing.last_feedback.unwrap().persistence_per_block, None);
        pacing.record(Duration::from_millis(200), 2);
        pacing.on_validation_completed(now + Duration::from_millis(10), true);
        assert_eq!(
            pacing.last_feedback.unwrap().persistence_per_block,
            Some(Duration::from_millis(100))
        );
        for number in 0..65 {
            pacing.record_local_feedback(alloy_primitives::B256::with_last_byte(number));
        }
        assert_eq!(pacing.local_feedback.len(), 64);
        assert!(pacing.local_feedback(alloy_primitives::B256::ZERO).is_none());
        let previous = pacing.last_feedback;
        pacing.on_validation_completed(now + Duration::from_millis(20), false);
        assert_eq!(pacing.last_feedback.unwrap().persistence_per_block, None);
        assert_eq!(pacing.local_feedback(alloy_primitives::B256::with_last_byte(64)), previous);
    }
    use super::PersistencePacing;
    use reth_primitives_traits::FastInstant as Instant;
    use std::time::Duration;

    #[test]
    fn persistence_pacing_counts_time_between_completions() {
        let mut pacing = PersistencePacing::default();
        for _ in 0..2 {
            pacing.record(Duration::from_millis(100), 1);
        }
        let start = Instant::now();
        assert_eq!(pacing.on_validation_completed(start, true), Duration::ZERO);
        // Below-threshold blocks still establish the next interval's start.
        assert_eq!(
            pacing.on_validation_completed(start + Duration::from_millis(10), false),
            Duration::ZERO
        );
        assert_eq!(
            pacing.on_validation_completed(start + Duration::from_millis(30), true),
            Duration::from_millis(88)
        );
        // Charge the actual sleep (including oversleep) only to the preceding block.
        pacing.total_wait += Duration::from_millis(110);
        assert_eq!(
            pacing.on_validation_completed(start + Duration::from_millis(160), true),
            Duration::from_millis(88)
        );
        pacing.total_wait += Duration::from_millis(100);
        // A below-threshold completion consumes the previous sleep accounting too.
        assert_eq!(
            pacing.on_validation_completed(start + Duration::from_millis(280), false),
            Duration::ZERO
        );
        assert_eq!(
            pacing.on_validation_completed(start + Duration::from_millis(300), true),
            Duration::from_millis(88)
        );
        pacing.total_wait += Duration::from_millis(100);
        // Genuine work/idle time that meets the estimate still needs no sleep.
        assert_eq!(
            pacing.on_validation_completed(start + Duration::from_millis(500), true),
            Duration::ZERO
        );
    }

    #[test]
    fn persistence_pacing_requires_two_saves_that_release_blocks() {
        let mut pacing = PersistencePacing::default();
        assert_eq!(pacing.delay(Duration::ZERO), Duration::ZERO);
        pacing.record(Duration::from_secs(1), 0);
        pacing.record(Duration::from_millis(100), 10);
        assert_eq!(pacing.delay(Duration::ZERO), Duration::ZERO);
        pacing.record(Duration::from_millis(200), 20);
        assert_eq!(pacing.delay(Duration::from_millis(4)), Duration::from_micros(6600));
        assert_eq!(pacing.delay(Duration::from_millis(10)), Duration::ZERO);
        assert_eq!(pacing.delay(Duration::from_millis(20)), Duration::ZERO);
    }

    #[test]
    fn persistence_pacing_weights_recent_saves_and_expires_old_samples() {
        let mut pacing = PersistencePacing::default();
        pacing.record(Duration::from_millis(110), 1);
        pacing.record(Duration::from_millis(220), 1);
        assert_eq!(pacing.delay(Duration::ZERO), Duration::from_micros(181_500));

        for _ in 0..3 {
            pacing.record(Duration::from_millis(10), 1);
        }
        assert_eq!(pacing.samples.len(), 3);
        assert_eq!(pacing.delay(Duration::ZERO), Duration::from_micros(11_000));
    }

    #[test]
    fn persistence_pacing_does_not_alternate_sleep_and_no_sleep() {
        let mut pacing = PersistencePacing::default();
        for _ in 0..2 {
            pacing.record(Duration::from_millis(500), 1);
        }
        let mut now = Instant::now();
        assert_eq!(pacing.on_validation_completed(now, true), Duration::ZERO);
        for _ in 0..20 {
            now += Duration::from_millis(200);
            let sleep = pacing.on_validation_completed(now, true);
            assert_eq!(sleep, Duration::from_millis(330));
            pacing.total_wait += sleep;
            now += sleep;
        }
    }
}

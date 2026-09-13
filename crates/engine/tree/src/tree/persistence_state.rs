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
use crossbeam_channel::Receiver as CrossbeamReceiver;
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

/// Paces validation using the last ten successful saves that released blocks from memory.
#[derive(Debug, Default)]
pub(crate) struct PersistencePacing {
    /// Oldest sample first; each sample is save duration per fully persisted block.
    samples: VecDeque<Duration>,
    /// Cumulative sleep, allowing an RPC to account for all blocks it connects.
    pub(crate) total_wait: Duration,
}

impl PersistencePacing {
    const WINDOW: usize = 10;
    const ALPHA: f64 = 2.0 / (Self::WINDOW as f64 + 1.0);

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
    pub(crate) fn delay(&self, validation_duration: Duration) -> Duration {
        if self.samples.len() < 2 {
            return Duration::ZERO;
        }
        let mut samples = self.samples.iter().map(Duration::as_secs_f64);
        let first = samples.next().expect("at least two samples");
        let ema =
            samples.fold(first, |ema, sample| Self::ALPHA * sample + (1.0 - Self::ALPHA) * ema);
        Duration::from_secs_f64(ema).saturating_sub(validation_duration)
    }
}

#[cfg(test)]
mod tests {
    use super::PersistencePacing;
    use std::time::Duration;

    #[test]
    fn persistence_pacing_requires_two_saves_that_release_blocks() {
        let mut pacing = PersistencePacing::default();
        assert_eq!(pacing.delay(Duration::ZERO), Duration::ZERO);
        pacing.record(Duration::from_secs(1), 0);
        pacing.record(Duration::from_millis(100), 10);
        assert_eq!(pacing.delay(Duration::ZERO), Duration::ZERO);
        pacing.record(Duration::from_millis(200), 20);
        assert_eq!(pacing.delay(Duration::from_millis(4)), Duration::from_millis(6));
        assert_eq!(pacing.delay(Duration::from_millis(10)), Duration::ZERO);
        assert_eq!(pacing.delay(Duration::from_millis(20)), Duration::ZERO);
    }

    #[test]
    fn persistence_pacing_weights_recent_saves_and_expires_old_samples() {
        let mut pacing = PersistencePacing::default();
        pacing.record(Duration::from_millis(110), 1);
        pacing.record(Duration::from_millis(220), 1);
        assert_eq!(pacing.delay(Duration::ZERO), Duration::from_millis(130));

        for _ in 0..10 {
            pacing.record(Duration::from_millis(10), 1);
        }
        assert_eq!(pacing.samples.len(), 10);
        assert_eq!(pacing.delay(Duration::ZERO), Duration::from_millis(10));
    }
}

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
use reth_primitives_traits::FastInstant as Instant;
use tracing::trace;

/// The state of the persistence task.
#[derive(Debug)]
pub struct PersistenceState {
    /// Hash and number of the last block persisted.
    ///
    /// This tracks the chain height that is persisted on disk
    pub(crate) last_persisted_block: BlockNumHash,
    /// Hash and number of the current canonical tip tracked by the tree.
    canonical_tip: BlockNumHash,
    /// Maximum canonical distance that can be executed ahead of persistence before we
    /// backpressure `reth_newPayload`.
    persistence_credit_cap: u64,
    /// Whether we should force persistence synchronization before processing more payloads.
    ///
    /// This is raised on durable-boundary transitions (for example reorg/finalization changes).
    enforce_durable_boundary: bool,
    /// Receiver end of channel where the result of the persistence task will be
    /// sent when done. A None value means there's no persistence task in progress.
    pub(crate) rx:
        Option<(CrossbeamReceiver<PersistenceResult>, Instant, CurrentPersistenceAction)>,
}

impl PersistenceState {
    /// Creates a new persistence state.
    pub(crate) const fn new(initial_block: BlockNumHash, persistence_credit_cap: u64) -> Self {
        Self {
            last_persisted_block: initial_block,
            canonical_tip: initial_block,
            persistence_credit_cap,
            enforce_durable_boundary: false,
            rx: None,
        }
    }

    /// Determines if there is a persistence task in progress by checking if the
    /// receiver is set.
    pub(crate) const fn in_progress(&self) -> bool {
        self.rx.is_some()
    }

    /// Returns the configured persistence credit cap.
    pub(crate) const fn persistence_credit_cap(&self) -> u64 {
        self.persistence_credit_cap
    }

    /// Returns canonical distance executed ahead of persisted state.
    pub(crate) const fn persistence_lag(&self) -> u64 {
        self.canonical_tip.number.saturating_sub(self.last_persisted_block.number)
    }

    /// Returns true if durable-boundary synchronization is pending.
    pub(crate) const fn should_force_durable_sync(&self) -> bool {
        self.enforce_durable_boundary
    }

    /// Marks that durable-boundary synchronization must happen before we continue hot-path
    /// payload processing.
    pub(crate) fn request_durable_sync(&mut self) {
        self.enforce_durable_boundary = self.persistence_lag() > 0;
    }

    /// Updates the tracked canonical tip.
    pub(crate) fn update_canonical_tip(&mut self, canonical_tip: BlockNumHash) {
        self.canonical_tip = canonical_tip;
        if self.enforce_durable_boundary && self.persistence_lag() == 0 {
            self.enforce_durable_boundary = false;
        }
    }

    /// Returns true if `reth_newPayload` should block on current persistence progress.
    pub(crate) fn should_block_reth_new_payload(&self) -> bool {
        let Some((_, _, action)) = self.rx.as_ref() else {
            return false;
        };

        if matches!(action, CurrentPersistenceAction::RemovingBlocks { .. }) {
            return true;
        }

        self.enforce_durable_boundary || self.persistence_lag() >= self.persistence_credit_cap
    }

    /// Sets the state for a block removal operation.
    pub(crate) fn start_remove(
        &mut self,
        new_tip_num: u64,
        rx: CrossbeamReceiver<PersistenceResult>,
    ) {
        self.enforce_durable_boundary = true;
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
        last_persisted_block_hash: B256,
        last_persisted_block_number: u64,
    ) {
        trace!(target: "engine::tree", block= %last_persisted_block_number, hash=%last_persisted_block_hash, "updating persistence state");
        self.rx = None;
        self.last_persisted_block =
            BlockNumHash::new(last_persisted_block_number, last_persisted_block_hash);
        if self.enforce_durable_boundary {
            self.enforce_durable_boundary = self.persistence_lag() > 0;
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::B256;

    #[test]
    fn tracks_lag_and_credit_window() {
        let initial = BlockNumHash::new(10, B256::ZERO);
        let mut state = PersistenceState::new(initial, 3);

        assert_eq!(state.persistence_lag(), 0);

        state.update_canonical_tip(BlockNumHash::new(12, B256::with_last_byte(0x12)));
        assert_eq!(state.persistence_lag(), 2);
        assert!(!state.should_block_reth_new_payload());

        let (_tx, rx) = crossbeam_channel::bounded(1);
        state.start_save(BlockNumHash::new(12, B256::with_last_byte(0x12)), rx);
        assert!(!state.should_block_reth_new_payload());

        state.update_canonical_tip(BlockNumHash::new(13, B256::with_last_byte(0x13)));
        assert_eq!(state.persistence_lag(), 3);
        assert!(state.should_block_reth_new_payload());
    }

    #[test]
    fn durable_boundary_forces_sync_until_caught_up() {
        let initial = BlockNumHash::new(20, B256::ZERO);
        let mut state = PersistenceState::new(initial, 4);
        state.update_canonical_tip(BlockNumHash::new(22, B256::with_last_byte(0x22)));

        state.request_durable_sync();
        assert!(state.should_force_durable_sync());

        let (_tx, rx) = crossbeam_channel::bounded(1);
        state.start_save(BlockNumHash::new(22, B256::with_last_byte(0x22)), rx);
        assert!(state.should_block_reth_new_payload());

        state.finish(B256::with_last_byte(0x21), 21);
        assert!(state.should_force_durable_sync());

        state.finish(B256::with_last_byte(0x22), 22);
        assert!(!state.should_force_durable_sync());
    }

    #[test]
    fn remove_actions_always_block_payloads() {
        let initial = BlockNumHash::new(30, B256::ZERO);
        let mut state = PersistenceState::new(initial, 8);

        let (_tx, rx) = crossbeam_channel::bounded(1);
        state.start_remove(29, rx);

        assert!(state.should_block_reth_new_payload());
    }
}

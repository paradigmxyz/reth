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

use crate::persistence::{PersistenceCanceller, PersistenceResult};
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
    /// Receiver end of channel where the result of the persistence task will be
    /// sent when done. A None value means there's no persistence task in progress.
    pub(crate) rx:
        Option<(CrossbeamReceiver<PersistenceResult>, Instant, CurrentPersistenceAction)>,
    /// Cancels the currently running block save, if any.
    pub(crate) save_canceller: Option<PersistenceCanceller>,
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
        self.save_canceller = None;
        self.rx =
            Some((rx, Instant::now(), CurrentPersistenceAction::RemovingBlocks { new_tip_num }));
    }

    /// Sets the state for a block save operation.
    pub(crate) fn start_save(
        &mut self,
        highest: BlockNumHash,
        canceller: PersistenceCanceller,
        rx: CrossbeamReceiver<PersistenceResult>,
    ) {
        self.save_canceller = Some(canceller);
        self.rx = Some((rx, Instant::now(), CurrentPersistenceAction::SavingBlocks { highest }));
    }

    /// Cancels the current block save if it includes a block at or above `first_invalidated`.
    pub(crate) fn cancel_save_if_intersects(&self, first_invalidated: u64) {
        if let Some((_, _, CurrentPersistenceAction::SavingBlocks { highest })) = &self.rx &&
            first_invalidated <= highest.number &&
            let Some(canceller) = &self.save_canceller
        {
            canceller.cancel();
        }
    }

    /// Returns the current persistence action. If there is no persistence task in progress, then
    /// this returns `None`.
    #[cfg(test)]
    pub(crate) fn current_action(&self) -> Option<&CurrentPersistenceAction> {
        self.rx.as_ref().map(|rx| &rx.2)
    }

    /// Clears the currently running persistence operation without advancing the persisted tip.
    pub(crate) fn finish_cancelled(&mut self) {
        self.rx = None;
        self.save_canceller = None;
    }

    /// Sets state for a finished persistence task.
    pub(crate) fn finish(
        &mut self,
        last_persisted_block_hash: B256,
        last_persisted_block_number: u64,
    ) {
        trace!(target: "engine::tree", block= %last_persisted_block_number, hash=%last_persisted_block_hash, "updating persistence state");
        self.rx = None;
        self.save_canceller = None;
        self.last_persisted_block =
            BlockNumHash::new(last_persisted_block_number, last_persisted_block_hash);
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

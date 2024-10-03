use alloy_eips::BlockNumHash;
use alloy_primitives::B256;
use std::{collections::VecDeque, time::Instant};
use tokio::sync::oneshot;
use tracing::{debug, trace};

/// The state of the persistence task.
#[derive(Default, Debug)]
pub struct PersistenceState {
    /// Hash and number of the last block persisted.
    ///
    /// This tracks the chain height that is persisted on disk
    pub(crate) last_persisted_block: BlockNumHash,
    /// Receiver end of channel where the result of the persistence task will be
    /// sent when done. A None value means there's no persistence task in progress.
    pub(crate) rx: Option<(oneshot::Receiver<Option<BlockNumHash>>, Instant)>,
    /// The block above which blocks should be removed from disk, because there has been an on disk
    /// reorg.
    pub(crate) remove_above_state: VecDeque<u64>,
}

impl PersistenceState {
    /// Determines if there is a persistence task in progress by checking if the
    /// receiver is set.
    pub(crate) const fn in_progress(&self) -> bool {
        self.rx.is_some()
    }

    /// Sets state for a started persistence task.
    pub(crate) fn start(&mut self, rx: oneshot::Receiver<Option<BlockNumHash>>) {
        self.rx = Some((rx, Instant::now()));
    }

    /// Sets the `remove_above_state`, to the new tip number specified, only if it is less than the
    /// current `last_persisted_block_number`.
    pub(crate) fn schedule_removal(&mut self, new_tip_num: u64) {
        debug!(target: "engine::tree", ?new_tip_num, prev_remove_state=?self.remove_above_state, last_persisted_block=?self.last_persisted_block, "Scheduling removal");
        self.remove_above_state.push_back(new_tip_num);
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
    }
}

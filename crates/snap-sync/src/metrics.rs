//! Metrics for a snap sync run.

use alloy_primitives::{B256, U256};
use reth_metrics::{
    metrics::{Counter, Gauge},
    Metrics,
};

// Progress and stalls of a snap sync run, updated only when a step commits or the run changes
// phase, never per account. Field docs are the exported metric descriptions.
#[derive(Clone, Metrics)]
#[metrics(scope = "sync.snap")]
pub(crate) struct SnapSyncMetrics {
    /// Current phase of the run: 0 waiting, 1 downloading, 2 catching up, 3 repairing, 4
    /// recovering, 5 verifying, 6 handed off.
    pub(crate) phase: Gauge,
    /// Block the downloaded state is anchored to.
    pub(crate) pivot_block: Gauge,
    /// Last block whose access list the downloaded state includes.
    pub(crate) applied_block: Gauge,
    /// Share of the account keyspace downloaded, from 0 to 1.
    pub(crate) account_progress: Gauge,
    /// Accounts a reorg left scheduled for repair.
    pub(crate) pending_repairs: Gauge,
    /// Storage responses committed, which advance while a large contract holds the account cursor.
    pub(crate) storage_responses: Counter,
    /// Pivots moved to a newer block.
    pub(crate) pivot_advances: Counter,
    /// Reorgs across the pivot repaired in place.
    pub(crate) reorg_recoveries: Counter,
    /// Attempts abandoned and started over.
    pub(crate) restarts: Counter,
    /// Waits for a block eligible as the pivot.
    pub(crate) pivot_waits: Counter,
    /// Waits for a connected peer serving snap/2.
    pub(crate) snap_peer_waits: Counter,
    /// Waits after a peer answered without the requested data.
    pub(crate) unavailable_waits: Counter,
    /// Waits after a transient request or header failure.
    pub(crate) transient_waits: Counter,
}

impl SnapSyncMetrics {
    pub(crate) fn set_phase(&self, phase: SnapPhase) {
        self.phase.set(phase as u8 as f64);
    }

    // Accounts download in key order, so the cursor's most significant limb gives the share done.
    pub(crate) fn set_account_progress(&self, next: Option<B256>) {
        let progress = next
            .map_or(1.0, |next| U256::from_be_bytes(next.0).as_limbs()[3] as f64 / u64::MAX as f64);
        self.account_progress.set(progress);
    }
}

// What a snap sync run is doing, as reported by `SnapSyncMetrics::phase`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum SnapPhase {
    // Waiting for headers, peers or data.
    Waiting = 0,
    // Downloading account ranges with their storage and code.
    Downloading = 1,
    // Applying block access lists up to the pivot.
    CatchingUp = 2,
    // Fetching accounts a reorg left scheduled for repair.
    Repairing = 3,
    // Repairing state after a reorg across the pivot.
    Recovering = 4,
    // Checking the downloaded state is complete before the trie rebuild.
    Verifying = 5,
    // The state is handed to the trie rebuild.
    HandedOff = 6,
}

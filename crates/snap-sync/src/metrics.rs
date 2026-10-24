//! Metrics for a snap sync run.

use alloy_primitives::{B256, U256};
use reth_metrics::{
    metrics::{Counter, Gauge},
    Metrics,
};

// Progress of a snap sync run, updated when a step commits or a stored attempt resumes.
#[derive(Clone, Metrics)]
#[metrics(scope = "sync.snap")]
pub(crate) struct SnapSyncMetrics {
    /// Block the downloaded state is anchored to.
    pub(crate) pivot_block: Gauge,
    /// Last block whose access list the downloaded state includes.
    pub(crate) applied_block: Gauge,
    /// Share of the account keyspace downloaded, from 0 to 1.
    pub(crate) account_progress: Gauge,
    /// Attempts abandoned and started over.
    pub(crate) restarts: Counter,
}

impl SnapSyncMetrics {
    // Drops the abandoned attempt's progress until the next one reports its own.
    pub(crate) fn clear_attempt(&self) {
        self.pivot_block.set(0.0);
        self.applied_block.set(0.0);
        self.account_progress.set(0.0);
    }

    // Accounts download in key order, so the cursor's most significant limb gives the share done.
    pub(crate) fn set_account_progress(&self, next: Option<B256>) {
        let progress = next
            .map_or(1.0, |next| U256::from_be_bytes(next.0).as_limbs()[3] as f64 / u64::MAX as f64);
        self.account_progress.set(progress);
    }
}

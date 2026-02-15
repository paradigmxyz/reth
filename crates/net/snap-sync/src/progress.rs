//! Snap sync progress tracking.
//!
//! Tracks the current phase and cursor positions to support resumability.

use alloy_primitives::{Address, B256};

/// Current phase of snap sync.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SnapPhase {
    /// Not started yet.
    #[default]
    Idle,
    /// Downloading account ranges.
    Accounts,
    /// Downloading storage slots for accounts with non-empty storage roots.
    Storages,
    /// Downloading contract bytecodes.
    Bytecodes,
    /// Building hashed state and verifying merkle root.
    Verification,
    /// Snap sync completed successfully.
    Done,
}

/// Tracks snap sync progress for resumability.
#[derive(Debug, Clone, Default)]
pub struct SnapProgress {
    /// The pivot block hash.
    pub pivot_hash: B256,
    /// The pivot block number.
    pub pivot_number: u64,
    /// The pivot block's state root.
    pub state_root: B256,
    /// Current sync phase.
    pub phase: SnapPhase,
    /// Account download cursor: next account hash to fetch.
    pub account_cursor: B256,
    /// Number of accounts downloaded so far.
    pub accounts_downloaded: u64,
    /// Storage download cursor: current account address being fetched.
    pub storage_account_cursor: Option<Address>,
    /// Storage slot cursor within the current account.
    pub storage_slot_cursor: B256,
    /// Number of storage slots downloaded so far.
    pub storage_slots_downloaded: u64,
    /// Number of bytecodes downloaded so far.
    pub bytecodes_downloaded: u64,
    /// Total number of bytecodes to download.
    pub bytecodes_total: u64,
}

impl SnapProgress {
    /// Creates a new progress tracker for the given pivot.
    pub fn new(pivot_hash: B256, pivot_number: u64, state_root: B256) -> Self {
        Self {
            pivot_hash,
            pivot_number,
            state_root,
            phase: SnapPhase::Accounts,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_progress_new() {
        let hash = B256::repeat_byte(0x01);
        let state_root = B256::repeat_byte(0x02);
        let progress = SnapProgress::new(hash, 100, state_root);
        assert_eq!(progress.pivot_hash, hash);
        assert_eq!(progress.pivot_number, 100);
        assert_eq!(progress.state_root, state_root);
        assert_eq!(progress.phase, SnapPhase::Accounts);
        assert_eq!(progress.accounts_downloaded, 0);
        assert_eq!(progress.storage_slots_downloaded, 0);
        assert_eq!(progress.bytecodes_downloaded, 0);
    }

    #[test]
    fn test_phase_default() {
        assert_eq!(SnapPhase::default(), SnapPhase::Idle);
    }
}

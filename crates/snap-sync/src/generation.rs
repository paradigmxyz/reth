//! Identifies one attempt at downloading state, and how far it has progressed.

use crate::{error::db_error, SnapSyncError};
use alloy_primitives::B256;
use reth_storage_api::HeaderProvider;

/// One attempt at downloading state, anchored to the pivot block it targets.
///
/// The anchor is kept as both a hash and a state root: the hash decides whether the attempt is
/// still on the canonical chain, and the root is what downloaded ranges authenticate against.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SnapGeneration {
    // Pivot block number.
    target_block: u64,
    // Pivot block hash.
    target_hash: B256,
    // Root downloaded ranges authenticate against.
    state_root: B256,
    // Stage reached so far.
    phase: SnapPhase,
    // First block whose block access list has not been applied.
    next_block: u64,
}

impl SnapGeneration {
    /// Creates a generation anchored to the given pivot, before any range is downloaded.
    pub const fn new(target_block: u64, target_hash: B256, state_root: B256) -> Self {
        Self {
            target_block,
            target_hash,
            state_root,
            phase: SnapPhase::Accounts,
            next_block: target_block.saturating_add(1),
        }
    }

    /// Pivot block number.
    pub const fn target_block(&self) -> u64 {
        self.target_block
    }

    /// Pivot block hash.
    pub const fn target_hash(&self) -> B256 {
        self.target_hash
    }

    /// State root that downloaded ranges authenticate against.
    pub const fn state_root(&self) -> B256 {
        self.state_root
    }

    /// Stage this generation has reached.
    pub const fn phase(&self) -> SnapPhase {
        self.phase
    }

    /// First block whose block access list has not been applied.
    pub const fn next_block(&self) -> u64 {
        self.next_block
    }

    /// Returns how far the canonical head has moved past this generation's anchor.
    pub const fn lag(&self, head: u64) -> u64 {
        head.saturating_sub(self.target_block)
    }

    /// Returns whether the block this generation is anchored to is still canonical.
    ///
    /// Separate from whether it is worth finishing: an orphaned anchor is recoverable from the
    /// abandoned branch's lists.
    pub fn is_canonical(&self, provider: &impl HeaderProvider) -> Result<bool, SnapSyncError> {
        let header = provider.sealed_header(self.target_block).map_err(db_error)?;
        Ok(header.is_some_and(|header| header.hash() == self.target_hash))
    }

    /// Returns this generation moved to `phase`.
    #[cfg(test)]
    pub(crate) const fn with_phase(mut self, phase: SnapPhase) -> Self {
        self.phase = phase;
        self
    }
}

/// The stage a [`SnapGeneration`] has reached.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapPhase {
    /// Account, storage and bytecode ranges are being downloaded.
    Accounts,
    /// Authenticated block access lists are being applied.
    BlockAccessLists,
    /// The final state trie is being rebuilt and checked.
    Trie,
}

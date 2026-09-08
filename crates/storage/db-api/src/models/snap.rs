//! Snap synchronization models.

use alloy_eips::BlockNumHash;
use alloy_primitives::B256;
use core::fmt;
use serde::{Deserialize, Serialize};

/// Encoding version of [`SnapAttempt`] written by this build.
pub const SNAP_ATTEMPT_VERSION: u32 = 1;

/// The snap synchronization attempt that owns the downloaded state.
///
/// Snap writes land in the canonical hashed state tables, so this record is what separates state a
/// live attempt is filling in from what an abandoned one left behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnapAttempt {
    /// Encoding version of this record.
    pub version: u32,
    /// Identity of this attempt.
    pub id: SnapAttemptId,
    /// Pivot block the downloaded state is anchored to.
    pub pivot: BlockNumHash,
    /// State root downloaded ranges authenticate against.
    pub state_root: B256,
    /// Bumped whenever the pivot moves, so responses proved against a superseded root are refused.
    pub state_version: u64,
    /// Whether the downloaded state has been verified.
    pub status: SnapBootstrapStatus,
}

impl SnapAttempt {
    /// Creates the record for an attempt superseding `previous`.
    pub const fn start(previous: Option<Self>, pivot: BlockNumHash, state_root: B256) -> Self {
        let id = match previous {
            Some(previous) => previous.id.next(),
            None => SnapAttemptId::FIRST,
        };
        Self {
            version: SNAP_ATTEMPT_VERSION,
            id,
            pivot,
            state_root,
            state_version: 0,
            status: SnapBootstrapStatus::Unfinished,
        }
    }

    /// Returns whether this attempt's state is still incomplete.
    pub const fn is_unfinished(&self) -> bool {
        matches!(self.status, SnapBootstrapStatus::Unfinished)
    }
}

/// Identity of one snap synchronization attempt.
///
/// A restart at the same pivot takes the next identity, so an abandoned attempt's leftover state
/// is never mistaken for the current attempt's work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SnapAttemptId(u64);

impl SnapAttemptId {
    /// Identity of a node's first attempt.
    pub const FIRST: Self = Self(0);

    /// Returns the identity superseding this one.
    pub const fn next(self) -> Self {
        Self(self.0.saturating_add(1))
    }
}

impl fmt::Display for SnapAttemptId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Whether a snap attempt's downloaded state has been verified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SnapBootstrapStatus {
    /// Downloads are outstanding, so the state is incomplete and is not canonical yet.
    Unfinished,
    /// The reconstructed trie root matched the target header.
    Verified,
}

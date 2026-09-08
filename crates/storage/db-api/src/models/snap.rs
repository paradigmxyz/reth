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
    // Encoding version of this record.
    version: u32,
    // Identity of this attempt.
    id: SnapAttemptId,
    // Pivot block the downloaded state is anchored to.
    pivot: BlockNumHash,
    // Root downloaded ranges authenticate against.
    state_root: B256,
    // Bumped whenever the pivot moves, so writes proved against a superseded root are refused.
    state_version: u64,
    // Whether the downloaded state has been verified.
    status: SnapBootstrapStatus,
}

impl SnapAttempt {
    /// Creates the record for an attempt taking identity `id`.
    pub const fn start(id: SnapAttemptId, pivot: BlockNumHash, state_root: B256) -> Self {
        Self {
            version: SNAP_ATTEMPT_VERSION,
            id,
            pivot,
            state_root,
            state_version: 0,
            status: SnapBootstrapStatus::Unfinished,
        }
    }

    /// Identity of this attempt.
    pub const fn id(&self) -> SnapAttemptId {
        self.id
    }

    /// Pivot block the downloaded state is anchored to.
    pub const fn pivot(&self) -> BlockNumHash {
        self.pivot
    }

    /// Root downloaded ranges authenticate against.
    pub const fn state_root(&self) -> B256 {
        self.state_root
    }

    /// Generation of the pivot this attempt is anchored to.
    pub const fn state_version(&self) -> u64 {
        self.state_version
    }

    /// Returns whether the downloaded state is still incomplete.
    pub const fn is_unfinished(&self) -> bool {
        matches!(self.status, SnapBootstrapStatus::Unfinished)
    }

    /// Re-anchors this attempt, superseding writes proved against the previous root.
    pub fn re_anchor(&mut self, pivot: BlockNumHash, state_root: B256) {
        self.pivot = pivot;
        self.state_root = state_root;
        self.state_version = self.state_version.saturating_add(1);
    }

    /// Marks the downloaded state verified.
    pub fn verify(&mut self) {
        self.status = SnapBootstrapStatus::Verified;
    }
}

/// Identity of one snap synchronization attempt.
///
/// Only ever handed out once, so an abandoned attempt's leftover state is never mistaken for the
/// current attempt's work.
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

// Whether a snap attempt's downloaded state has been verified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum SnapBootstrapStatus {
    // Downloads are outstanding, so the state is incomplete and is not canonical yet.
    Unfinished,
    // The reconstructed trie root matched the target header.
    Verified,
}

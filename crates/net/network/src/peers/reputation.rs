//! Peer reputation management

/// The type that tracks the reputation score.
type Reputation = i32;

/// The reputation value below which new connection from/to peers are rejected.
pub const BANNED_REPUTATION: i32 = 0;

/// The reputation change to apply to a node that dropped the connection.
const REMOTE_DISCONNECT_REPUTATION_CHANGE: i32 = -100;

/// Represents a change in a peer's reputation.
#[derive(Debug, Copy, Clone, Default)]
pub(crate) struct ReputationChange(Reputation);

// === impl ReputationChange ===

impl ReputationChange {
    /// Apply no reputation change.
    pub(crate) const fn none() -> Self {
        Self(0)
    }

    /// Reputation change for a peer that dropped the connection.
    pub(crate) const fn dropped() -> Self {
        Self(REMOTE_DISCONNECT_REPUTATION_CHANGE)
    }

    /// Helper type for easier conversion
    #[inline]
    pub(crate) fn as_i32(self) -> Reputation {
        self.0
    }
}

impl From<ReputationChange> for Reputation {
    fn from(value: ReputationChange) -> Self {
        value.0
    }
}

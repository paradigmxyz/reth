//! Peer reputation management

use reth_network_api::{Reputation, ReputationChangeKind};

/// The default reputation of a peer
pub(crate) const DEFAULT_REPUTATION: Reputation = 0;

/// The minimal unit we're measuring reputation
const REPUTATION_UNIT: i32 = -1024;

/// The reputation value below which new connection from/to peers are rejected.
pub(crate) const BANNED_REPUTATION: i32 = 50 * REPUTATION_UNIT;

/// The reputation change to apply to a peer that dropped the connection.
const REMOTE_DISCONNECT_REPUTATION_CHANGE: i32 = 4 * REPUTATION_UNIT;

/// The reputation change to apply to a peer that we failed to connect to.
const FAILED_TO_CONNECT_REPUTATION_CHANGE: i32 = 25 * REPUTATION_UNIT;

/// The reputation change to apply to a peer that failed to respond in time.
const TIMEOUT_REPUTATION_CHANGE: i32 = 4 * REPUTATION_UNIT;

/// The reputation change to apply to a peer that sent a bad message.
const BAD_MESSAGE_REPUTATION_CHANGE: i32 = 16 * REPUTATION_UNIT;

/// The reputation change applies to a peer that has sent a transaction (full or hash) that we
/// already know about and have already previously received from that peer.
///
/// Note: this appears to be quite common in practice, so by default this is 0, which doesn't
/// apply any changes to the peer's reputation, effectively ignoring it.
const ALREADY_SEEN_TRANSACTION_REPUTATION_CHANGE: i32 = 0;

/// The reputation change to apply to a peer which violates protocol rules: minimal reputation
const BAD_PROTOCOL_REPUTATION_CHANGE: i32 = i32::MIN;

/// The reputation change to apply to a peer that sent a bad announcement.
// todo: current value is a hint, needs to be set properly
const BAD_ANNOUNCEMENT_REPUTATION_CHANGE: i32 = REPUTATION_UNIT;

/// Returns `true` if the given reputation is below the [`BANNED_REPUTATION`] threshold
#[inline]
pub(crate) fn is_banned_reputation(reputation: i32) -> bool {
    reputation < BANNED_REPUTATION
}

/// How the [`ReputationChangeKind`] are weighted.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct ReputationChangeWeights {
    /// Weight for [`ReputationChangeKind::BadMessage`]
    pub bad_message: Reputation,
    /// Weight for [`ReputationChangeKind::BadBlock`]
    pub bad_block: Reputation,
    /// Weight for [`ReputationChangeKind::BadTransactions`]
    pub bad_transactions: Reputation,
    /// Weight for [`ReputationChangeKind::AlreadySeenTransaction`]
    pub already_seen_transactions: Reputation,
    /// Weight for [`ReputationChangeKind::Timeout`]
    pub timeout: Reputation,
    /// Weight for [`ReputationChangeKind::BadProtocol`]
    pub bad_protocol: Reputation,
    /// Weight for [`ReputationChangeKind::FailedToConnect`]
    pub failed_to_connect: Reputation,
    /// Weight for [`ReputationChangeKind::Dropped`]
    pub dropped: Reputation,
    /// Weight for [`ReputationChangeKind::BadAnnouncement`]
    pub bad_announcement: Reputation,
}

// === impl ReputationChangeWeights ===

impl ReputationChangeWeights {
    /// Returns the quantifiable [`ReputationChange`] for the given [`ReputationChangeKind`] using
    /// the configured weights
    pub(crate) fn change(&self, kind: ReputationChangeKind) -> ReputationChange {
        match kind {
            ReputationChangeKind::BadMessage => self.bad_message.into(),
            ReputationChangeKind::BadBlock => self.bad_block.into(),
            ReputationChangeKind::BadTransactions => self.bad_transactions.into(),
            ReputationChangeKind::AlreadySeenTransaction => self.already_seen_transactions.into(),
            ReputationChangeKind::Timeout => self.timeout.into(),
            ReputationChangeKind::BadProtocol => self.bad_protocol.into(),
            ReputationChangeKind::FailedToConnect => self.failed_to_connect.into(),
            ReputationChangeKind::Dropped => self.dropped.into(),
            ReputationChangeKind::Reset => DEFAULT_REPUTATION.into(),
            ReputationChangeKind::Other(val) => val.into(),
            ReputationChangeKind::BadAnnouncement => self.bad_announcement.into(),
        }
    }
}

impl Default for ReputationChangeWeights {
    fn default() -> Self {
        Self {
            bad_block: BAD_MESSAGE_REPUTATION_CHANGE,
            bad_transactions: BAD_MESSAGE_REPUTATION_CHANGE,
            already_seen_transactions: ALREADY_SEEN_TRANSACTION_REPUTATION_CHANGE,
            bad_message: BAD_MESSAGE_REPUTATION_CHANGE,
            timeout: TIMEOUT_REPUTATION_CHANGE,
            bad_protocol: BAD_PROTOCOL_REPUTATION_CHANGE,
            failed_to_connect: FAILED_TO_CONNECT_REPUTATION_CHANGE,
            dropped: REMOTE_DISCONNECT_REPUTATION_CHANGE,
            bad_announcement: BAD_ANNOUNCEMENT_REPUTATION_CHANGE,
        }
    }
}

/// Represents a change in a peer's reputation.
#[derive(Debug, Copy, Clone, Default)]
pub(crate) struct ReputationChange(Reputation);

// === impl ReputationChange ===

impl ReputationChange {
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

impl From<Reputation> for ReputationChange {
    fn from(value: Reputation) -> Self {
        ReputationChange(value)
    }
}

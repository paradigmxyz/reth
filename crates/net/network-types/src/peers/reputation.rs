//! Peer reputation management

/// The default reputation of a peer
pub const DEFAULT_REPUTATION: Reputation = 0;

/// The minimal unit we're measuring reputation
const REPUTATION_UNIT: i32 = -1024;

/// The reputation value below which new connection from/to peers are rejected.
pub const BANNED_REPUTATION: i32 = 50 * REPUTATION_UNIT;

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

/// The maximum reputation change that can be applied to a trusted peer.
/// This is used to prevent a single bad message from a trusted peer to cause a significant change.
/// This gives a trusted peer more leeway when interacting with the node, which is useful for in
/// custom setups. By not setting this to `0` we still allow trusted peer penalization but less than
/// untrusted peers.
pub const MAX_TRUSTED_PEER_REPUTATION_CHANGE: Reputation = 2 * REPUTATION_UNIT;

/// Returns `true` if the given reputation is below the [`BANNED_REPUTATION`] threshold
#[inline]
pub const fn is_banned_reputation(reputation: i32) -> bool {
    reputation < BANNED_REPUTATION
}

/// The type that tracks the reputation score.
pub type Reputation = i32;

/// Various kinds of reputation changes.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ReputationChangeKind {
    /// Received an unspecific bad message from the peer
    BadMessage,
    /// Peer sent a bad block.
    ///
    /// Note: this will we only used in pre-merge, pow consensus, since after no more block announcements are sent via devp2p: [EIP-3675](https://eips.ethereum.org/EIPS/eip-3675#devp2p)
    BadBlock,
    /// Peer sent a bad transaction message. E.g. Transactions which weren't recoverable.
    BadTransactions,
    /// Peer sent a bad announcement message, e.g. invalid transaction type for the configured
    /// network.
    BadAnnouncement,
    /// Peer sent a message that included a hash or transaction that we already received from the
    /// peer.
    ///
    /// According to the [Eth spec](https://github.com/ethereum/devp2p/blob/master/caps/eth.md):
    ///
    /// > A node should never send a transaction back to a peer that it can determine already knows
    /// > of it (either because it was previously sent or because it was informed from this peer
    /// > originally). This is usually achieved by remembering a set of transaction hashes recently
    /// > relayed by the peer.
    AlreadySeenTransaction,
    /// Peer failed to respond in time.
    Timeout,
    /// Peer does not adhere to network protocol rules.
    BadProtocol,
    /// Failed to establish a connection to the peer.
    FailedToConnect,
    /// Connection dropped by peer.
    Dropped,
    /// Reset the reputation to the default value.
    Reset,
    /// Apply a reputation change by value
    Other(Reputation),
}

impl ReputationChangeKind {
    /// Returns true if the reputation change is a [`ReputationChangeKind::Reset`].
    pub const fn is_reset(&self) -> bool {
        matches!(self, Self::Reset)
    }

    /// Returns true if the reputation change is [`ReputationChangeKind::Dropped`].
    pub const fn is_dropped(&self) -> bool {
        matches!(self, Self::Dropped)
    }
}

/// How the [`ReputationChangeKind`] are weighted.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// Creates a new instance that doesn't penalize any kind of reputation change.
    pub const fn zero() -> Self {
        Self {
            bad_block: 0,
            bad_transactions: 0,
            already_seen_transactions: 0,
            bad_message: 0,
            timeout: 0,
            bad_protocol: 0,
            failed_to_connect: 0,
            dropped: 0,
            bad_announcement: 0,
        }
    }

    /// Returns the quantifiable [`ReputationChange`] for the given [`ReputationChangeKind`] using
    /// the configured weights
    pub fn change(&self, kind: ReputationChangeKind) -> ReputationChange {
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
pub struct ReputationChange(Reputation);

// === impl ReputationChange ===

impl ReputationChange {
    /// Helper type for easier conversion
    #[inline]
    pub const fn as_i32(self) -> Reputation {
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
        Self(value)
    }
}

/// Outcomes when a reputation change is applied to a peer
#[derive(Debug, Clone, Copy)]
pub enum ReputationChangeOutcome {
    /// Nothing to do.
    None,
    /// Ban the peer.
    Ban,
    /// Ban and disconnect
    DisconnectAndBan,
    /// Unban the peer
    Unban,
}

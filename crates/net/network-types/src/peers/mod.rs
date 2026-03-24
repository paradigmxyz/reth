pub mod addr;
pub mod config;
pub mod kind;
pub mod reputation;
pub mod state;

pub use config::{ConnectionsConfig, PeersConfig};
pub use reputation::{Reputation, ReputationChange, ReputationChangeKind, ReputationChangeWeights};

use alloy_eip2124::ForkId;
use reth_network_peers::{NodeRecord, PeerId};
use tracing::trace;

use crate::{
    is_banned_reputation, PeerAddr, PeerConnectionState, PeerKind, ReputationChangeOutcome,
    DEFAULT_REPUTATION,
};

/// Tracks info about a single peer.
#[derive(Debug, Clone)]
pub struct Peer {
    /// Where to reach the peer.
    pub addr: PeerAddr,
    /// Reputation of the peer.
    pub reputation: i32,
    /// The state of the connection, if any.
    pub state: PeerConnectionState,
    /// The [`ForkId`] that the peer announced via discovery.
    pub fork_id: Option<Box<ForkId>>,
    /// Whether the entry should be removed after an existing session was terminated.
    pub remove_after_disconnect: bool,
    /// The kind of peer
    pub kind: PeerKind,
    /// Whether the peer is currently backed off.
    pub backed_off: bool,
    /// Counts number of times the peer was backed off due to a severe
    /// [`BackoffKind`](crate::BackoffKind).
    pub severe_backoff_counter: u8,
}

// === impl Peer ===

impl Peer {
    /// Returns a new peer for given [`PeerAddr`].
    pub fn new(addr: PeerAddr) -> Self {
        Self::with_state(addr, Default::default())
    }

    /// Returns a new trusted peer for given [`PeerAddr`].
    pub fn trusted(addr: PeerAddr) -> Self {
        Self { kind: PeerKind::Trusted, ..Self::new(addr) }
    }

    /// Returns the reputation of the peer
    pub const fn reputation(&self) -> i32 {
        self.reputation
    }

    /// Returns a new peer for given [`PeerAddr`] and [`PeerConnectionState`].
    pub fn with_state(addr: PeerAddr, state: PeerConnectionState) -> Self {
        Self {
            addr,
            state,
            reputation: DEFAULT_REPUTATION,
            fork_id: None,
            remove_after_disconnect: false,
            kind: Default::default(),
            backed_off: false,
            severe_backoff_counter: 0,
        }
    }

    /// Returns a new peer for given [`PeerAddr`] and [`PeerKind`].
    pub fn with_kind(addr: PeerAddr, kind: PeerKind) -> Self {
        Self { kind, ..Self::new(addr) }
    }

    /// Resets the reputation of the peer to the default value. This always returns
    /// [`ReputationChangeOutcome::None`].
    pub const fn reset_reputation(&mut self) -> ReputationChangeOutcome {
        self.reputation = DEFAULT_REPUTATION;

        ReputationChangeOutcome::None
    }

    /// Applies a reputation change to the peer and returns what action should be taken.
    pub fn apply_reputation(
        &mut self,
        reputation: i32,
        kind: ReputationChangeKind,
    ) -> ReputationChangeOutcome {
        let previous = self.reputation;
        // we add reputation since negative reputation change decrease total reputation
        self.reputation = previous.saturating_add(reputation);

        trace!(target: "net::peers", reputation=%self.reputation, banned=%self.is_banned(), ?kind, "applied reputation change");

        if self.state.is_connected() && self.is_banned() {
            self.state.disconnect();
            return ReputationChangeOutcome::DisconnectAndBan
        }

        if self.is_banned() && !is_banned_reputation(previous) {
            return ReputationChangeOutcome::Ban
        }

        if !self.is_banned() && is_banned_reputation(previous) {
            return ReputationChangeOutcome::Unban
        }

        ReputationChangeOutcome::None
    }

    /// Returns true if the peer's reputation is below the banned threshold.
    #[inline]
    pub const fn is_banned(&self) -> bool {
        is_banned_reputation(self.reputation)
    }

    /// Returns `true` if peer is banned.
    #[inline]
    pub const fn is_backed_off(&self) -> bool {
        self.backed_off
    }

    /// Unbans the peer by resetting its reputation
    #[inline]
    pub const fn unban(&mut self) {
        self.reputation = DEFAULT_REPUTATION
    }

    /// Returns whether this peer is trusted
    #[inline]
    pub const fn is_trusted(&self) -> bool {
        matches!(self.kind, PeerKind::Trusted)
    }

    /// Returns whether this peer is static
    #[inline]
    pub const fn is_static(&self) -> bool {
        matches!(self.kind, PeerKind::Static)
    }
}

/// Peer info persisted to disk.
///
/// Contains richer metadata than a plain [`NodeRecord`], preserving the peer's kind, fork ID,
/// and reputation across restarts.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PersistedPeerInfo {
    /// The node record (id, address, ports).
    pub record: NodeRecord,
    /// The kind of peer.
    pub kind: PeerKind,
    /// The [`ForkId`] that the peer announced via discovery.
    #[cfg_attr(feature = "serde", serde(default, skip_serializing_if = "Option::is_none"))]
    pub fork_id: Option<ForkId>,
    /// The peer's reputation at the time of persisting.
    pub reputation: i32,
}

impl PersistedPeerInfo {
    /// Returns the peer id.
    pub const fn peer_id(&self) -> PeerId {
        self.record.id
    }

    /// Converts a legacy [`NodeRecord`] into a [`PersistedPeerInfo`] with default metadata.
    pub const fn from_node_record(record: NodeRecord) -> Self {
        Self { record, kind: PeerKind::Basic, fork_id: None, reputation: DEFAULT_REPUTATION }
    }
}

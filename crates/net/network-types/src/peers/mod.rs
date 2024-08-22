pub mod addr;
pub mod config;
pub mod kind;
pub mod reputation;
pub mod state;

pub use config::{ConnectionsConfig, PeersConfig};
pub use reputation::{Reputation, ReputationChange, ReputationChangeKind, ReputationChangeWeights};

use reth_ethereum_forks::ForkId;
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
    pub fork_id: Option<ForkId>,
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
    pub fn reset_reputation(&mut self) -> ReputationChangeOutcome {
        self.reputation = DEFAULT_REPUTATION;

        ReputationChangeOutcome::None
    }

    /// Applies a reputation change to the peer and returns what action should be taken.
    pub fn apply_reputation(&mut self, reputation: i32) -> ReputationChangeOutcome {
        let previous = self.reputation;
        // we add reputation since negative reputation change decrease total reputation
        self.reputation = previous.saturating_add(reputation);

        trace!(target: "net::peers", reputation=%self.reputation, banned=%self.is_banned(), "applied reputation change");

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
    pub fn unban(&mut self) {
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

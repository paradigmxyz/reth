use futures::StreamExt;
use reth_eth_wire::DisconnectReason;
use reth_primitives::PeerId;
use std::{
    collections::{hash_map::Entry, HashMap, VecDeque},
    net::SocketAddr,
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    sync::mpsc,
    time::{Instant, Interval},
};
use tokio_stream::wrappers::UnboundedReceiverStream;

/// The reputation value below which new connection from/to peers are rejected.
pub const BANNED_REPUTATION: i32 = 0;

/// The reputation change to apply to a node that dropped the connection.
const REMOTE_DISCONNECT_REPUTATION_CHANGE: i32 = -100;

/// A communication channel to the [`PeersManager`] to apply changes to the peer set.
pub struct PeersHandle {
    /// Sender half of command channel back to the [`PeersManager`]
    manager_tx: mpsc::UnboundedSender<PeerCommand>,
}

/// Maintains the state of _all_ the peers known to the network.
///
/// This is supposed to be owned by the network itself, but can be reached via the [`PeersHandle`].
/// From this type, connections to peers are established or disconnected, see [`PeerAction`].
///
/// The [`PeersManager`] will be notified on peer related changes
pub(crate) struct PeersManager {
    /// All peers known to the network
    peers: HashMap<PeerId, Peer>,
    /// Copy of the receiver half, so new [`PeersHandle`] can be created on demand.
    manager_tx: mpsc::UnboundedSender<PeerCommand>,
    /// Receiver half of the command channel.
    handle_rx: UnboundedReceiverStream<PeerCommand>,
    /// Buffered actions until the manager is polled.
    queued_actions: VecDeque<PeerAction>,
    /// Interval for triggering connections if there are free slots.
    refill_slots_interval: Interval,
    /// Tracks current slot stats.
    connection_info: ConnectionInfo,
}

impl PeersManager {
    /// Create a new instance with the given config
    pub(crate) fn new(config: PeersConfig) -> Self {
        let PeersConfig { refill_slots_interval, connection_info } = config;
        let (manager_tx, handle_rx) = mpsc::unbounded_channel();
        Self {
            peers: Default::default(),
            manager_tx,
            handle_rx: UnboundedReceiverStream::new(handle_rx),
            queued_actions: Default::default(),
            refill_slots_interval: tokio::time::interval_at(
                Instant::now() + refill_slots_interval,
                refill_slots_interval,
            ),
            connection_info,
        }
    }

    /// Returns a new [`PeersHandle`] that can send commands to this type.
    pub(crate) fn handle(&self) -> PeersHandle {
        PeersHandle { manager_tx: self.manager_tx.clone() }
    }

    /// Called when a new _incoming_ active session was established to the given peer.
    ///
    /// This will update the state of the peer if not yet tracked.
    ///
    /// If the reputation of the peer is below the `BANNED_REPUTATION` threshold, a disconnect will
    /// be scheduled.
    pub(crate) fn on_active_session(&mut self, peer_id: PeerId, addr: SocketAddr) {
        match self.peers.entry(peer_id) {
            Entry::Occupied(mut entry) => {
                let value = entry.get_mut();
                if value.is_banned() {
                    self.queued_actions.push_back(PeerAction::DisconnectBannedIncoming { peer_id });
                    return
                }
                value.state = PeerConnectionState::In;
            }
            Entry::Vacant(entry) => {
                entry.insert(Peer::with_state(addr, PeerConnectionState::In));
            }
        }

        // keep track of new connection
        self.connection_info.inc_in();
    }

    /// Called when a session to a peer was disconnected.
    ///
    /// Accepts an additional [`ReputationChange`] value to apply to the peer.
    pub(crate) fn on_disconnected(&mut self, peer: PeerId, reputation_change: ReputationChange) {
        if let Some(mut peer) = self.peers.get_mut(&peer) {
            self.connection_info.decr_state(peer.state);
            peer.state = PeerConnectionState::Idle;
            peer.reputation -= reputation_change.0;
        }
    }

    /// Called for a newly discovered peer.
    ///
    /// If the peer already exists, then the address will e updated. If the addresses differ, the
    /// old address is returned
    pub(crate) fn add_discovered_node(&mut self, peer_id: PeerId, addr: SocketAddr) {
        match self.peers.entry(peer_id) {
            Entry::Occupied(mut entry) => {
                let node = entry.get_mut();
                node.addr = addr;
            }
            Entry::Vacant(entry) => {
                entry.insert(Peer::new(addr));
            }
        }
    }

    /// Removes the tracked node from the set.
    pub(crate) fn remove_discovered_node(&mut self, peer_id: PeerId) {
        if let Some(entry) = self.peers.remove(&peer_id) {
            if entry.state.is_connected() {
                // TODO(mattsse): is this right to disconnect peers?
                self.connection_info.decr_state(entry.state);
                self.queued_actions.push_back(PeerAction::Disconnect { peer_id, reason: None })
            }
        }
    }

    /// Returns the idle peer with the highest reputation.
    ///
    /// Returns `None` if no peer is available.
    fn best_unconnected(&mut self) -> Option<(PeerId, &mut Peer)> {
        self.peers
            .iter_mut()
            .filter(|(_, peer)| peer.state.is_unconnected())
            .fold(None::<(&PeerId, &mut Peer)>, |mut best_peer, candidate| {
                if let Some(best_peer) = best_peer.take() {
                    if best_peer.1.reputation >= candidate.1.reputation {
                        return Some(best_peer)
                    }
                }
                Some(candidate)
            })
            .map(|(id, peer)| (*id, peer))
    }

    /// If there's capacity for new outbound connections, this will queue new
    /// [`PeerAction::Connect`] actions.
    ///
    /// New connections are only initiated, if slots are available and appropriate peers are
    /// available.
    fn fill_outbound_slots(&mut self) {
        // as long as there a slots available try to fill them with the best peers
        while self.connection_info.has_out_capacity() {
            let action = {
                let (peer_id, peer) = match self.best_unconnected() {
                    Some(peer) => peer,
                    _ => break,
                };

                // If best peer does not meet reputation threshold exit immediately.
                if peer.is_banned() {
                    break
                }
                peer.state = PeerConnectionState::Out;
                PeerAction::Connect { peer_id, remote_addr: peer.addr }
            };

            self.connection_info.inc_out();
            self.queued_actions.push_back(action);
        }
    }

    /// Advances the state.
    ///
    /// Event hooks invoked externally may trigger a new [`PeerAction`] that are buffered until
    /// [`PeersManager::poll_next`] is called.
    pub fn poll(&mut self, cx: &mut Context<'_>) -> Poll<PeerAction> {
        loop {
            // drain buffered actions
            if let Some(action) = self.queued_actions.pop_front() {
                return Poll::Ready(action)
            }

            while let Poll::Ready(Some(cmd)) = self.handle_rx.poll_next_unpin(cx) {
                match cmd {
                    PeerCommand::Add { peer_id, addr } => {
                        self.add_discovered_node(peer_id, addr);
                    }
                    PeerCommand::Remove(peer) => self.remove_discovered_node(peer),
                }
            }

            if self.refill_slots_interval.poll_tick(cx).is_ready() {
                self.fill_outbound_slots();
            }

            if self.queued_actions.is_empty() {
                return Poll::Pending
            }
        }
    }
}

/// Tracks stats about connected nodes
#[derive(Debug)]
pub struct ConnectionInfo {
    /// Currently occupied slots for outbound connections.
    num_outbound: usize,
    /// Currently occupied slots for inbound connections.
    num_inbound: usize,
    /// Maximum allowed outbound connections.
    max_outbound: usize,
    /// Maximum allowed inbound connections.
    max_inbound: usize,
}

// === impl ConnectionInfo ===

impl ConnectionInfo {
    ///  Returns `true` if there's still capacity for a new outgoing connection.
    fn has_out_capacity(&self) -> bool {
        self.num_outbound < self.max_outbound
    }

    fn decr_state(&mut self, state: PeerConnectionState) {
        match state {
            PeerConnectionState::Idle => {}
            PeerConnectionState::In => self.decr_in(),
            PeerConnectionState::Out => self.decr_out(),
        }
    }

    fn decr_out(&mut self) {
        self.num_outbound -= 1;
    }

    fn inc_out(&mut self) {
        self.num_outbound += 1;
    }

    fn inc_in(&mut self) {
        self.num_inbound += 1;
    }

    fn decr_in(&mut self) {
        self.num_inbound -= 1;
    }
}

/// Tracks info about a single peer.
struct Peer {
    /// Where to reach the peer
    addr: SocketAddr,
    /// Reputation of the peer.
    reputation: i32,
    /// The state of the connection, if any.
    state: PeerConnectionState,
}

// === impl Peer ===

impl Peer {
    fn new(addr: SocketAddr) -> Self {
        Self::with_state(addr, Default::default())
    }

    fn with_state(addr: SocketAddr, state: PeerConnectionState) -> Self {
        Self { addr, state, reputation: 0 }
    }

    /// Returns true if the peer's reputation is below the banned threshold.
    #[inline]
    fn is_banned(&self) -> bool {
        self.reputation < BANNED_REPUTATION
    }
}

/// Represents the kind of connection established to the peer, if any
#[derive(Debug, Clone, Copy, Default)]
enum PeerConnectionState {
    /// Not connected currently.
    #[default]
    Idle,
    /// Connected via incoming connection.
    In,
    /// Connected via outgoing connection.
    Out,
}

// === impl PeerConnectionState ===

impl PeerConnectionState {
    /// Returns whether we're currently connected with this peer
    #[inline]
    fn is_connected(&self) -> bool {
        matches!(self, PeerConnectionState::In | PeerConnectionState::Out)
    }

    /// Returns if there's currently no connection to that peer.
    #[inline]
    fn is_unconnected(&self) -> bool {
        matches!(self, PeerConnectionState::Idle)
    }
}

/// Represents a change in a peer's reputation.
#[derive(Debug, Copy, Clone, Default)]
pub(crate) struct ReputationChange(i32);

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
}

/// Commands the [`PeersManager`] listens for.
pub(crate) enum PeerCommand {
    /// Command for manually add
    Add {
        /// Identifier of the peer.
        peer_id: PeerId,
        /// The address of the peer
        addr: SocketAddr,
    },
    /// Remove a peer from the set
    ///
    /// If currently connected this will disconnect the sessin
    Remove(PeerId),
}

/// Actions the peer manager can trigger.
#[derive(Debug)]
pub enum PeerAction {
    /// Start a new connection to a peer.
    Connect {
        /// The peer to connect to.
        peer_id: PeerId,
        /// Where to reach the node
        remote_addr: SocketAddr,
    },
    /// Disconnect an existing connection.
    Disconnect { peer_id: PeerId, reason: Option<DisconnectReason> },
    /// Disconnect an existing incoming connection, because the peers reputation is below the
    /// banned threshold.
    DisconnectBannedIncoming {
        /// Peer id of the established connection.
        peer_id: PeerId,
    },
}

/// Config type for initiating a [`PeersManager`] instance
#[derive(Debug)]
pub struct PeersConfig {
    /// How even to recheck free slots for outbound connections
    pub refill_slots_interval: Duration,
    /// Restrictions on connections
    pub connection_info: ConnectionInfo,
}

impl Default for PeersConfig {
    fn default() -> Self {
        Self {
            refill_slots_interval: Duration::from_millis(1_000),
            connection_info: ConnectionInfo {
                num_outbound: 0,
                num_inbound: 0,
                max_outbound: 70,
                max_inbound: 30,
            },
        }
    }
}

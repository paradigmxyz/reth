use crate::{
    error::{error_merits_discovery_ban, is_fatal_protocol_error},
    peers::{reputation::BANNED_REPUTATION, ReputationChangeKind, ReputationChangeWeights},
};
use futures::StreamExt;
use reth_eth_wire::{error::EthStreamError, DisconnectReason};
use reth_net_common::ban_list::BanList;
use reth_primitives::PeerId;
use std::{
    collections::{hash_map::Entry, HashMap, VecDeque},
    fmt::Display,
    net::{IpAddr, SocketAddr},
    task::{Context, Poll},
    time::Duration,
};
use thiserror::Error;
use tokio::{
    sync::mpsc,
    time::{Instant, Interval},
};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::trace;

/// A communication channel to the [`PeersManager`] to apply manual changes to the peer set.
#[derive(Clone, Debug)]
pub struct PeersHandle {
    /// Sender half of command channel back to the [`PeersManager`]
    manager_tx: mpsc::UnboundedSender<PeerCommand>,
}

// === impl PeersHandle ===

impl PeersHandle {
    fn send(&self, cmd: PeerCommand) {
        let _ = self.manager_tx.send(cmd);
    }

    /// Adds a peer to the set.
    pub fn add_peer(&self, peer_id: PeerId, addr: SocketAddr) {
        self.send(PeerCommand::Add(peer_id, addr));
    }

    /// Removes a peer from the set.
    pub fn remove_peer(&self, peer_id: PeerId) {
        self.send(PeerCommand::Remove(peer_id));
    }

    /// Send a reputation change for the given peer.
    pub fn reputation_change(&self, peer_id: PeerId, kind: ReputationChangeKind) {
        self.send(PeerCommand::ReputationChange(peer_id, kind));
    }
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
    /// How to weigh reputation changes
    reputation_weights: ReputationChangeWeights,
    /// Tracks current slot stats.
    connection_info: ConnectionInfo,
    /// Tracks unwanted ips/peer ids,
    ban_list: BanList,
}

impl PeersManager {
    /// Create a new instance with the given config
    pub(crate) fn new(config: PeersConfig) -> Self {
        let PeersConfig { refill_slots_interval, connection_info, reputation_weights, ban_list } =
            config;
        let (manager_tx, handle_rx) = mpsc::unbounded_channel();
        Self {
            peers: Default::default(),
            manager_tx,
            handle_rx: UnboundedReceiverStream::new(handle_rx),
            queued_actions: Default::default(),
            reputation_weights,
            refill_slots_interval: tokio::time::interval_at(
                Instant::now() + refill_slots_interval,
                refill_slots_interval,
            ),
            connection_info,
            ban_list,
        }
    }

    /// Returns a new [`PeersHandle`] that can send commands to this type.
    pub(crate) fn handle(&self) -> PeersHandle {
        PeersHandle { manager_tx: self.manager_tx.clone() }
    }

    /// Invoked when a new _incoming_ tcp connection is accepted.
    ///
    /// returns an error if the inbound ip address is on the ban list or
    /// we have reached our limit for max inbound connections
    pub(crate) fn on_inbound_pending_session(
        &mut self,
        addr: IpAddr,
    ) -> Result<(), InboundConnectionError> {
        if self.ban_list.is_banned_ip(&addr) {
            return Err(InboundConnectionError::IpBanned)
        }
        if !self.connection_info.has_in_capacity() {
            return Err(InboundConnectionError::ExceedsLimit(self.connection_info.max_inbound))
        }

        // keep track of new connection
        self.connection_info.inc_in();
        Ok(())
    }

    /// Invoked when a pending session was closed.
    pub(crate) fn on_closed_incoming_pending_session(&mut self) {
        self.connection_info.decr_in()
    }

    /// Invoked when a pending session was closed.
    pub(crate) fn on_closed_outgoing_pending_session(&mut self) {
        self.connection_info.decr_out()
    }

    /// Called when a new _incoming_ active session was established to the given peer.
    ///
    /// This will update the state of the peer if not yet tracked.
    ///
    /// If the reputation of the peer is below the `BANNED_REPUTATION` threshold, a disconnect will
    /// be scheduled.
    pub(crate) fn on_active_inbound_session(&mut self, peer_id: PeerId, addr: SocketAddr) {
        // we only need to check the peer id here as the ip address will have been checked at
        // on_inbound_pending_session
        if self.ban_list.is_banned_peer(&peer_id) {
            self.queued_actions.push_back(PeerAction::DisconnectBannedIncoming { peer_id });
            return
        }

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
    }

    /// Apply the corresponding reputation change to the given peer
    pub(crate) fn apply_reputation_change(&mut self, peer_id: &PeerId, rep: ReputationChangeKind) {
        let reputation_change = self.reputation_weights.change(rep);
        let should_disconnect = if let Some(mut peer) = self.peers.get_mut(peer_id) {
            peer.reputation = peer.reputation.saturating_sub(reputation_change.as_i32());
            let should_disconnect = peer.state.is_connected() && peer.is_banned();

            if should_disconnect {
                peer.state.disconnect();
            }

            should_disconnect
        } else {
            false
        };

        if should_disconnect {
            // start the disconnect process
            self.queued_actions
                .push_back(PeerAction::Disconnect { peer_id: *peer_id, reason: None })
        }
    }

    /// Gracefully disconnected
    pub(crate) fn on_disconnected(&mut self, peer_id: &PeerId) {
        if let Some(mut peer) = self.peers.get_mut(peer_id) {
            self.connection_info.decr_state(peer.state);
            peer.state = PeerConnectionState::Idle;
        }

        self.fill_outbound_slots();
    }

    /// Called when a session to a peer was forcefully disconnected.
    ///
    /// Depending on whether the error is fatal, the peer will be removed from the peer set
    /// otherwise its reputation is slashed.
    pub(crate) fn on_connection_dropped(
        &mut self,
        remote_addr: &SocketAddr,
        peer_id: &PeerId,
        err: &EthStreamError,
    ) {
        if is_fatal_protocol_error(err) {
            // remove the peer to which we can't establish a connection due to protocol related
            // issues.
            if let Some(peer) = self.peers.remove(peer_id) {
                self.connection_info.decr_state(peer.state);
            }

            // If the error is caused by a peer that should be banned
            if error_merits_discovery_ban(err) {
                self.queued_actions.push_back(PeerAction::DiscoveryBan {
                    peer_id: *peer_id,
                    ip_addr: remote_addr.ip(),
                })
            }
        } else if let Some(mut peer) = self.peers.get_mut(peer_id) {
            self.connection_info.decr_state(peer.state);
            peer.state = PeerConnectionState::Idle;
            let reputation_change = self.reputation_weights.change(ReputationChangeKind::Dropped);
            peer.reputation = peer.reputation.saturating_sub(reputation_change.as_i32());
        }

        self.fill_outbound_slots();
    }

    /// Called for a newly discovered peer.
    ///
    /// If the peer already exists, then the address will be updated. If the addresses differ, the
    /// old address is returned
    pub(crate) fn add_discovered_node(&mut self, peer_id: PeerId, addr: SocketAddr) {
        if self.ban_list.is_banned(&peer_id, &addr.ip()) {
            return
        }

        match self.peers.entry(peer_id) {
            Entry::Occupied(mut entry) => {
                let node = entry.get_mut();
                node.addr = addr;
                return
            }
            Entry::Vacant(entry) => {
                trace!(target : "net::peers", ?peer_id, ?addr, "discovered new node");
                entry.insert(Peer::new(addr));
            }
        }

        self.fill_outbound_slots();
    }

    /// Removes the tracked node from the set.
    pub(crate) fn remove_discovered_node(&mut self, peer_id: PeerId) {
        if let Some(entry) = self.peers.remove(&peer_id) {
            trace!(target : "net::peers",  ?peer_id, "remove discovered node");
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
        let mut unconnected = self.peers.iter_mut().filter(|(_, peer)| peer.state.is_unconnected());

        // keep track of the best peer, if there's one
        let mut best_peer = unconnected.next()?;

        for maybe_better in unconnected {
            if maybe_better.1.reputation > best_peer.1.reputation {
                best_peer = maybe_better;
            }
        }
        Some((*best_peer.0, best_peer.1))
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

                trace!(target : "net::peers",  ?peer_id, addr=?peer.addr, "schedule outbound connection");

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
                    PeerCommand::Add(peer_id, addr) => {
                        self.add_discovered_node(peer_id, addr);
                    }
                    PeerCommand::Remove(peer) => self.remove_discovered_node(peer),
                    PeerCommand::ReputationChange(peer_id, rep) => {
                        self.apply_reputation_change(&peer_id, rep)
                    }
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
    /// Counter for currently occupied slots for active outbound connections.
    num_outbound: usize,
    /// Counter for currently occupied slots for active inbound connections.
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

    ///  Returns `true` if there's still capacity for a new incoming connection.
    fn has_in_capacity(&self) -> bool {
        self.num_inbound < self.max_inbound
    }

    fn decr_state(&mut self, state: PeerConnectionState) {
        match state {
            PeerConnectionState::Idle => {}
            PeerConnectionState::DisconnectingIn | PeerConnectionState::In => self.decr_in(),
            PeerConnectionState::DisconnectingOut | PeerConnectionState::Out => self.decr_out(),
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

impl Default for ConnectionInfo {
    fn default() -> Self {
        ConnectionInfo { num_outbound: 0, num_inbound: 0, max_outbound: 100, max_inbound: 30 }
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
    /// Disconnect of an incoming connection in progress
    DisconnectingIn,
    /// Disconnect of an outgoing connection in progress
    DisconnectingOut,
    /// Connected via incoming connection.
    In,
    /// Connected via outgoing connection.
    Out,
}

// === impl PeerConnectionState ===

impl PeerConnectionState {
    /// Sets the disconnect state
    #[inline]
    fn disconnect(&mut self) {
        match self {
            PeerConnectionState::In => *self = PeerConnectionState::DisconnectingIn,
            PeerConnectionState::Out => *self = PeerConnectionState::DisconnectingOut,
            _ => {}
        }
    }

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

/// Commands the [`PeersManager`] listens for.
pub(crate) enum PeerCommand {
    /// Command for manually add
    Add(PeerId, SocketAddr),
    /// Remove a peer from the set
    ///
    /// If currently connected this will disconnect the session
    Remove(PeerId),
    /// Apply a reputation change to the given peer.
    ReputationChange(PeerId, ReputationChangeKind),
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
    /// banned threshold or is on the [`BanList`]
    DisconnectBannedIncoming {
        /// Peer id of the established connection.
        peer_id: PeerId,
    },
    /// Ban the peer in discovery.
    DiscoveryBan { peer_id: PeerId, ip_addr: IpAddr },
}

/// Config type for initiating a [`PeersManager`] instance
#[derive(Debug)]
pub struct PeersConfig {
    /// How even to recheck free slots for outbound connections
    pub refill_slots_interval: Duration,
    /// Restrictions on connections
    pub connection_info: ConnectionInfo,
    /// How to weigh reputation changes
    pub reputation_weights: ReputationChangeWeights,
    /// Restrictions on PeerIds and Ips
    pub ban_list: BanList,
}

impl Default for PeersConfig {
    fn default() -> Self {
        Self {
            refill_slots_interval: Duration::from_millis(1_000),
            connection_info: Default::default(),
            reputation_weights: Default::default(),
            ban_list: Default::default(),
        }
    }
}

impl PeersConfig {
    /// A set of peer_ids and ip addr that we want to never connect to
    pub fn with_ban_list(mut self, ban_list: BanList) -> Self {
        self.ban_list = ban_list;
        self
    }

    /// Maximum occupied slots for outbound connections.
    pub fn with_max_pending_outbound(mut self, num_outbound: usize) -> Self {
        self.connection_info.num_outbound = num_outbound;
        self
    }

    /// Maximum occupied slots for inbound connections.
    pub fn with_max_pending_inbound(mut self, num_inbound: usize) -> Self {
        self.connection_info.num_inbound = num_inbound;
        self
    }
    /// Maximum allowed outbound connections.
    pub fn with_max_outbound(mut self, max_outbound: usize) -> Self {
        self.connection_info.max_outbound = max_outbound;
        self
    }
    /// Maximum allowed inbound connections.
    pub fn with_max_inbound(mut self, max_inbound: usize) -> Self {
        self.connection_info.max_inbound = max_inbound;
        self
    }

    /// How often to recheck free slots for outbound connections
    pub fn with_slot_refill_interval(mut self, interval: Duration) -> Self {
        self.refill_slots_interval = interval;
        self
    }
}

#[derive(Debug, Error)]
pub enum InboundConnectionError {
    ExceedsLimit(usize),
    IpBanned,
}

impl Display for InboundConnectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

#[cfg(test)]
mod test {
    use super::PeersManager;
    use crate::{
        peers::{
            manager::{ConnectionInfo, PeerConnectionState},
            PeerAction,
        },
        PeersConfig,
    };
    use reth_net_common::ban_list::BanList;
    use reth_primitives::{PeerId, H512};
    use std::{
        collections::HashSet,
        net::{IpAddr, Ipv4Addr, SocketAddr},
    };

    #[tokio::test]
    async fn test_discovery_ban_list() {
        let ip = IpAddr::V4(Ipv4Addr::new(127, 0, 1, 2));
        let socket_addr = SocketAddr::new(ip, 8008);
        let ban_list = BanList::new(HashSet::new(), vec![ip]);
        let config = PeersConfig::default().with_ban_list(ban_list);
        let mut peer_manager = PeersManager::new(config);
        peer_manager.add_discovered_node(H512::default(), socket_addr);

        assert!(peer_manager.peers.is_empty());
    }

    #[tokio::test]
    async fn test_on_pending_ban_list() {
        let ip = IpAddr::V4(Ipv4Addr::new(127, 0, 1, 2));
        let socket_addr = SocketAddr::new(ip, 8008);
        let ban_list = BanList::new(HashSet::new(), vec![ip]);
        let config = PeersConfig::default().with_ban_list(ban_list);
        let mut peer_manager = PeersManager::new(config);
        let a = peer_manager.on_inbound_pending_session(socket_addr.ip());
        // because we have no active peers this should be fine for testings
        match a {
            Ok(_) => panic!(),
            Err(err) => match err {
                super::InboundConnectionError::IpBanned {} => {}
                super::InboundConnectionError::ExceedsLimit { .. } => {
                    panic!()
                }
            },
        }
    }

    #[tokio::test]
    async fn test_on_active_inbound_ban_list() {
        let ip = IpAddr::V4(Ipv4Addr::new(127, 0, 1, 2));
        let socket_addr = SocketAddr::new(ip, 8008);
        let given_peer_id: PeerId = H512::from_low_u64_ne(123403423412);
        let ban_list = BanList::new(vec![given_peer_id], HashSet::new());
        let config = PeersConfig::default().with_ban_list(ban_list);
        let mut peer_manager = PeersManager::new(config);
        peer_manager.on_active_inbound_session(given_peer_id, socket_addr);

        let Some(PeerAction::DisconnectBannedIncoming { peer_id }) = peer_manager.queued_actions.pop_front() else { panic!() };

        assert_eq!(peer_id, given_peer_id)
    }

    #[test]
    fn test_connection_limits() {
        let mut info = ConnectionInfo::default();
        info.inc_in();
        assert_eq!(info.num_inbound, 1);
        assert_eq!(info.num_outbound, 0);
        assert!(info.has_in_capacity());

        info.decr_in();
        assert_eq!(info.num_inbound, 0);
        assert_eq!(info.num_outbound, 0);

        info.inc_out();
        assert_eq!(info.num_inbound, 0);
        assert_eq!(info.num_outbound, 1);
        assert!(info.has_out_capacity());

        info.decr_out();
        assert_eq!(info.num_inbound, 0);
        assert_eq!(info.num_outbound, 0);
    }

    #[test]
    fn test_connection_peer_state() {
        let mut info = ConnectionInfo::default();
        info.inc_in();

        info.decr_state(PeerConnectionState::In);
        assert_eq!(info.num_inbound, 0);
        assert_eq!(info.num_outbound, 0);

        info.inc_out();

        info.decr_state(PeerConnectionState::Out);
        assert_eq!(info.num_inbound, 0);
        assert_eq!(info.num_outbound, 0);
    }
}

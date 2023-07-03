use crate::{
    error::{BackoffKind, SessionError},
    peers::{
        reputation::{is_banned_reputation, DEFAULT_REPUTATION},
        ReputationChangeWeights, DEFAULT_MAX_PEERS_INBOUND, DEFAULT_MAX_PEERS_OUTBOUND,
    },
    session::{Direction, PendingSessionHandshakeError},
};
use futures::StreamExt;
use reth_eth_wire::{errors::EthStreamError, DisconnectReason};
use reth_net_common::ban_list::BanList;
use reth_network_api::{PeerKind, ReputationChangeKind};
use reth_primitives::{ForkId, NodeRecord, PeerId};
use std::{
    collections::{hash_map::Entry, HashMap, HashSet, VecDeque},
    fmt::Display,
    io::{self, ErrorKind},
    net::{IpAddr, SocketAddr},
    path::Path,
    task::{Context, Poll},
    time::Duration,
};
use thiserror::Error;
use tokio::{
    sync::{mpsc, oneshot},
    time::{Instant, Interval},
};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::{debug, info, trace};

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

    /// Returns a peer by its [`PeerId`], or `None` if the peer is not in the peer set.
    pub async fn peer_by_id(&self, peer_id: PeerId) -> Option<Peer> {
        let (tx, rx) = oneshot::channel();
        self.send(PeerCommand::GetPeer(peer_id, tx));

        rx.await.unwrap_or(None)
    }

    /// Returns all peers in the peerset.
    pub async fn all_peers(&self) -> Vec<NodeRecord> {
        let (tx, rx) = oneshot::channel();
        self.send(PeerCommand::GetPeers(tx));

        rx.await.unwrap_or_default()
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
    /// Copy of the sender half, so new [`PeersHandle`] can be created on demand.
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
    /// Tracks unwanted ips/peer ids.
    ban_list: BanList,
    /// Tracks currently backed off peers.
    backed_off_peers: HashMap<PeerId, std::time::Instant>,
    /// Interval at which to check for peers to unban and release from the backoff map.
    release_interval: Interval,
    /// How long to ban bad peers.
    ban_duration: Duration,
    /// How long peers to which we could not connect for non-fatal reasons, e.g.
    /// [`DisconnectReason::TooManyPeers`], are put in time out.
    backoff_durations: PeerBackoffDurations,
    /// If non-trusted peers should be connected to
    connect_trusted_nodes_only: bool,
    /// Timestamp of the last time [Self::tick] was called.
    last_tick: Instant,
}

impl PeersManager {
    /// Create a new instance with the given config
    pub(crate) fn new(config: PeersConfig) -> Self {
        let PeersConfig {
            refill_slots_interval,
            connection_info,
            reputation_weights,
            ban_list,
            ban_duration,
            backoff_durations,
            trusted_nodes,
            connect_trusted_nodes_only,
            basic_nodes,
            ..
        } = config;
        let (manager_tx, handle_rx) = mpsc::unbounded_channel();
        let now = Instant::now();

        // We use half of the interval to decrease the max duration to `150%` in worst case
        let unban_interval = ban_duration.min(backoff_durations.low) / 2;

        let mut peers = HashMap::with_capacity(trusted_nodes.len() + basic_nodes.len());

        for NodeRecord { address, tcp_port, udp_port: _, id } in trusted_nodes {
            peers.entry(id).or_insert_with(|| Peer::trusted(SocketAddr::from((address, tcp_port))));
        }

        for NodeRecord { address, tcp_port, udp_port: _, id } in basic_nodes {
            peers.entry(id).or_insert_with(|| Peer::new(SocketAddr::from((address, tcp_port))));
        }

        Self {
            peers,
            manager_tx,
            handle_rx: UnboundedReceiverStream::new(handle_rx),
            queued_actions: Default::default(),
            reputation_weights,
            refill_slots_interval: tokio::time::interval_at(
                now + refill_slots_interval,
                refill_slots_interval,
            ),
            release_interval: tokio::time::interval_at(now + unban_interval, unban_interval),
            connection_info,
            ban_list,
            backed_off_peers: Default::default(),
            ban_duration,
            backoff_durations,
            connect_trusted_nodes_only,
            last_tick: Instant::now(),
        }
    }

    /// Returns a new [`PeersHandle`] that can send commands to this type.
    pub(crate) fn handle(&self) -> PeersHandle {
        PeersHandle { manager_tx: self.manager_tx.clone() }
    }

    /// Returns the number of peers in the peer set
    #[inline]
    pub(crate) fn num_known_peers(&self) -> usize {
        self.peers.len()
    }

    /// Returns an iterator over all peers
    pub(crate) fn iter_peers(&self) -> impl Iterator<Item = NodeRecord> + '_ {
        self.peers.iter().map(|(peer_id, v)| NodeRecord::new(v.addr, *peer_id))
    }

    /// Returns the number of currently active inbound connections.
    #[inline]
    pub(crate) fn num_inbound_connections(&self) -> usize {
        self.connection_info.num_inbound
    }

    /// Returns the number of currently active outbound connections.
    #[inline]
    pub(crate) fn num_outbound_connections(&self) -> usize {
        self.connection_info.num_outbound
    }

    /// Returns the number of currently backed off peers.
    #[inline]
    pub(crate) fn num_backed_off_peers(&self) -> usize {
        self.backed_off_peers.len()
    }

    /// Invoked when a new _incoming_ tcp connection is accepted.
    ///
    /// returns an error if the inbound ip address is on the ban list or
    /// we have reached our limit for max inbound connections
    pub(crate) fn on_incoming_pending_session(
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

    /// Invoked when a previous call to [Self::on_incoming_pending_session] succeeded but it was
    /// rejected.
    pub(crate) fn on_incoming_pending_session_rejected_internally(&mut self) {
        self.connection_info.decr_in();
    }

    /// Invoked when a pending session was closed.
    pub(crate) fn on_incoming_pending_session_gracefully_closed(&mut self) {
        self.connection_info.decr_in()
    }

    /// Invoked when a pending session was closed.
    pub(crate) fn on_incoming_pending_session_dropped(
        &mut self,
        remote_addr: SocketAddr,
        err: &PendingSessionHandshakeError,
    ) {
        if err.is_fatal_protocol_error() {
            self.ban_ip(remote_addr.ip());

            if err.merits_discovery_ban() {
                self.queued_actions
                    .push_back(PeerAction::DiscoveryBanIp { ip_addr: remote_addr.ip() })
            }
        }

        self.connection_info.decr_in()
    }

    /// Called when a new _incoming_ active session was established to the given peer.
    ///
    /// This will update the state of the peer if not yet tracked.
    ///
    /// If the reputation of the peer is below the `BANNED_REPUTATION` threshold, a disconnect will
    /// be scheduled.
    pub(crate) fn on_incoming_session_established(&mut self, peer_id: PeerId, addr: SocketAddr) {
        // we only need to check the peer id here as the ip address will have been checked at
        // on_inbound_pending_session. We also check if the peer is in the backoff list here.
        if self.ban_list.is_banned_peer(&peer_id) {
            self.queued_actions.push_back(PeerAction::DisconnectBannedIncoming { peer_id });
            return
        }

        // start a new tick, so the peer is not immediately rewarded for the time since last tick
        self.tick();

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
                // peer is missing in the table, we add it but mark it as to be removed after
                // disconnect, because we only know the outgoing port
                let mut peer = Peer::with_state(addr, PeerConnectionState::In);
                peer.remove_after_disconnect = true;
                entry.insert(peer);
                self.queued_actions.push_back(PeerAction::PeerAdded(peer_id));
            }
        }
    }

    /// Bans the peer temporarily with the configured ban timeout
    fn ban_peer(&mut self, peer_id: PeerId) {
        self.ban_list.ban_peer_until(peer_id, std::time::Instant::now() + self.ban_duration);
        self.queued_actions.push_back(PeerAction::BanPeer { peer_id });
    }

    /// Bans the IP temporarily with the configured ban timeout
    fn ban_ip(&mut self, ip: IpAddr) {
        self.ban_list.ban_ip_until(ip, std::time::Instant::now() + self.ban_duration);
    }

    /// Temporarily puts the peer in timeout
    fn backoff_peer_until(&mut self, peer_id: PeerId, until: std::time::Instant) {
        trace!(target: "net::peers", ?peer_id, "backing off");

        if let Some(peer) = self.peers.get_mut(&peer_id) {
            peer.backed_off = true;
            self.backed_off_peers.insert(peer_id, until);
        }
    }

    /// Unbans the peer
    fn unban_peer(&mut self, peer_id: PeerId) {
        self.ban_list.unban_peer(&peer_id);
        self.queued_actions.push_back(PeerAction::UnBanPeer { peer_id });
    }

    /// Tick function to update reputation of all connected peers.
    /// Peers are rewarded with reputation increases for the time they are connected since the last
    /// tick. This is to prevent peers from being disconnected eventually due to slashed
    /// reputation because of some bad messages (most likely transaction related)
    fn tick(&mut self) {
        let now = Instant::now();
        // Determine the number of seconds since the last tick.
        // Ensuring that now is always greater than last_tick to account for issues with system
        // time.
        let secs_since_last_tick =
            if self.last_tick > now { 0 } else { (now - self.last_tick).as_secs() as i32 };
        self.last_tick = now;

        // update reputation via seconds connected
        for peer in self.peers.iter_mut().filter(|(_, peer)| peer.state.is_connected()) {
            // update reputation via seconds connected, but keep the target _around_ the default
            // reputation.
            if peer.1.reputation < DEFAULT_REPUTATION {
                peer.1.reputation += secs_since_last_tick;
            }
        }
    }

    pub(crate) fn get_reputation(&self, peer_id: &PeerId) -> Option<i32> {
        self.peers.get(peer_id).map(|peer| peer.reputation)
    }

    /// Apply the corresponding reputation change to the given peer
    pub(crate) fn apply_reputation_change(&mut self, peer_id: &PeerId, rep: ReputationChangeKind) {
        let outcome = if let Some(peer) = self.peers.get_mut(peer_id) {
            // First check if we should reset the reputation
            if rep.is_reset() {
                peer.reset_reputation()
            } else {
                let reputation_change = self.reputation_weights.change(rep);
                peer.apply_reputation(reputation_change.as_i32())
            }
        } else {
            return
        };

        match outcome {
            ReputationChangeOutcome::None => {}
            ReputationChangeOutcome::Ban => {
                self.ban_peer(*peer_id);
            }
            ReputationChangeOutcome::Unban => self.unban_peer(*peer_id),
            ReputationChangeOutcome::DisconnectAndBan => {
                self.queued_actions.push_back(PeerAction::Disconnect {
                    peer_id: *peer_id,
                    reason: Some(DisconnectReason::DisconnectRequested),
                });
                self.ban_peer(*peer_id);
            }
        }
    }

    /// Gracefully disconnected a pending session
    pub(crate) fn on_pending_session_gracefully_closed(&mut self, peer_id: &PeerId) {
        if let Some(peer) = self.peers.get_mut(peer_id) {
            peer.state = PeerConnectionState::Idle;
        } else {
            return
        }
        self.connection_info.decr_out()
    }

    /// Invoked when a pending outgoing session was closed during authentication or the handshake.
    pub(crate) fn on_pending_session_dropped(
        &mut self,
        remote_addr: &SocketAddr,
        peer_id: &PeerId,
        err: &PendingSessionHandshakeError,
    ) {
        self.on_connection_failure(remote_addr, peer_id, err, ReputationChangeKind::FailedToConnect)
    }

    /// Gracefully disconnected an active session
    pub(crate) fn on_active_session_gracefully_closed(&mut self, peer_id: PeerId) {
        match self.peers.entry(peer_id) {
            Entry::Occupied(mut entry) => {
                self.connection_info.decr_state(entry.get().state);

                if entry.get().remove_after_disconnect && !entry.get().is_trusted() {
                    // this peer should be removed from the set
                    entry.remove();
                    self.queued_actions.push_back(PeerAction::PeerRemoved(peer_id));
                } else {
                    // reset the peer's state
                    // we reset the backoff counter since we're able to establish a successful
                    // session to that peer
                    entry.get_mut().severe_backoff_counter = 0;
                    entry.get_mut().state = PeerConnectionState::Idle;
                    return
                }
            }
            Entry::Vacant(_) => return,
        }

        self.fill_outbound_slots();
    }

    /// Called when an _active_ session to a peer was forcefully dropped due to an error.
    ///
    /// Depending on whether the error is fatal, the peer will be removed from the peer set
    /// otherwise its reputation is slashed.
    pub(crate) fn on_active_session_dropped(
        &mut self,
        remote_addr: &SocketAddr,
        peer_id: &PeerId,
        err: &EthStreamError,
    ) {
        self.on_connection_failure(remote_addr, peer_id, err, ReputationChangeKind::Dropped)
    }

    /// Called when an attempt to create a pending session failed while setting up a tcp connection.
    pub(crate) fn on_outgoing_connection_failure(
        &mut self,
        remote_addr: &SocketAddr,
        peer_id: &PeerId,
        err: &io::Error,
    ) {
        self.on_connection_failure(remote_addr, peer_id, err, ReputationChangeKind::FailedToConnect)
    }

    fn on_connection_failure(
        &mut self,
        remote_addr: &SocketAddr,
        peer_id: &PeerId,
        err: impl SessionError,
        reputation_change: ReputationChangeKind,
    ) {
        trace!(target: "net::peers", ?remote_addr, ?peer_id, ?err, "handling failed connection");

        if err.is_fatal_protocol_error() {
            trace!(target: "net::peers", ?remote_addr, ?peer_id, ?err, "fatal connection error");
            // remove the peer to which we can't establish a connection due to protocol related
            // issues.
            if let Some(peer) = self.peers.remove(peer_id) {
                self.connection_info.decr_state(peer.state);
                self.queued_actions.push_back(PeerAction::PeerRemoved(*peer_id));
            }

            // ban the peer
            self.ban_peer(*peer_id);

            // If the error is caused by a peer that should be banned from discovery
            if err.merits_discovery_ban() {
                self.queued_actions.push_back(PeerAction::DiscoveryBanPeerId {
                    peer_id: *peer_id,
                    ip_addr: remote_addr.ip(),
                })
            }
        } else {
            let mut backoff_until = None;

            if let Some(peer) = self.peers.get_mut(peer_id) {
                if let Some(kind) = err.should_backoff() {
                    // Increment peer.backoff_counter
                    if kind.is_severe() {
                        peer.severe_backoff_counter += 1;
                    }

                    let backoff_time =
                        self.backoff_durations.backoff_until(kind, peer.severe_backoff_counter);

                    // The peer has signaled that it is currently unable to process any more
                    // connections, so we will hold off on attempting any new connections for a
                    // while
                    backoff_until = Some(backoff_time);
                } else {
                    // If the error was not a backoff error, we reduce the peer's reputation
                    let reputation_change = self.reputation_weights.change(reputation_change);
                    peer.reputation = peer.reputation.saturating_add(reputation_change.as_i32());
                };

                self.connection_info.decr_state(peer.state);
                peer.state = PeerConnectionState::Idle;
            }
            if let Some(backoff_until) = backoff_until {
                self.backoff_peer_until(*peer_id, backoff_until);
            }
        }

        self.fill_outbound_slots();
    }

    /// Invoked if a session was disconnected because there's already a connection to the peer.
    ///
    /// If the session was an outgoing connection, this means that the peer initiated a connection
    /// to us at the same time and this connection is already established.
    pub(crate) fn on_already_connected(&mut self, direction: Direction) {
        match direction {
            Direction::Incoming => {}
            Direction::Outgoing(_) => {
                // need to decrement the outgoing counter
                self.connection_info.decr_out();
            }
        }
    }

    /// Called as follow-up for a discovered peer.
    ///
    /// The [`ForkId`] is retrieved from an ENR record that the peer announces over the discovery
    /// protocol
    pub(crate) fn set_discovered_fork_id(&mut self, peer_id: PeerId, fork_id: ForkId) {
        if let Some(peer) = self.peers.get_mut(&peer_id) {
            trace!(target : "net::peers", ?peer_id, ?fork_id, "set discovered fork id");
            peer.fork_id = Some(fork_id);
        }
    }

    /// Called for a newly discovered peer.
    ///
    /// If the peer already exists, then the address, kind and fork_id will be updated.
    pub(crate) fn add_peer(&mut self, peer_id: PeerId, addr: SocketAddr, fork_id: Option<ForkId>) {
        self.add_peer_kind(peer_id, PeerKind::Basic, addr, fork_id)
    }

    /// Called for a newly discovered trusted peer.
    ///
    /// If the peer already exists, then the address and kind will be updated.
    #[allow(dead_code)]
    pub(crate) fn add_trusted_peer(&mut self, peer_id: PeerId, addr: SocketAddr) {
        self.add_peer_kind(peer_id, PeerKind::Trusted, addr, None)
    }

    /// Called for a newly discovered peer.
    ///
    /// If the peer already exists, then the address, kind and fork_id will be updated.
    pub(crate) fn add_peer_kind(
        &mut self,
        peer_id: PeerId,
        kind: PeerKind,
        addr: SocketAddr,
        fork_id: Option<ForkId>,
    ) {
        if self.ban_list.is_banned(&peer_id, &addr.ip()) {
            return
        }

        match self.peers.entry(peer_id) {
            Entry::Occupied(mut entry) => {
                let peer = entry.get_mut();
                peer.kind = kind;
                peer.fork_id = fork_id;
                peer.addr = addr;

                if peer.state.is_incoming() {
                    // now that we have an actual discovered address, for that peer and not just the
                    // ip of the incoming connection, we don't need to remove the peer after
                    // disconnecting, See `on_incoming_session_established`
                    peer.remove_after_disconnect = false;
                }

                return
            }
            Entry::Vacant(entry) => {
                trace!(target : "net::peers", ?peer_id, ?addr, "discovered new node");
                let mut peer = Peer::with_kind(addr, kind);
                peer.fork_id = fork_id;
                entry.insert(peer);
                self.queued_actions.push_back(PeerAction::PeerAdded(peer_id));
            }
        }

        self.fill_outbound_slots();
    }

    /// Removes the tracked node from the set.
    pub(crate) fn remove_peer(&mut self, peer_id: PeerId) {
        let Entry::Occupied(entry) = self.peers.entry(peer_id) else { return };
        if entry.get().is_trusted() {
            return
        }
        let mut peer = entry.remove();

        trace!(target : "net::peers",  ?peer_id, "remove discovered node");
        self.queued_actions.push_back(PeerAction::PeerRemoved(peer_id));

        if peer.state.is_connected() {
            debug!(target : "net::peers",  ?peer_id, "disconnecting on remove from discovery");
            // we terminate the active session here, but only remove the peer after the session
            // was disconnected, this prevents the case where the session is scheduled for
            // disconnect but the node is immediately rediscovered, See also
            // [`Self::on_disconnected()`]
            peer.remove_after_disconnect = true;
            peer.state.disconnect();
            self.peers.insert(peer_id, peer);
            self.queued_actions.push_back(PeerAction::Disconnect {
                peer_id,
                reason: Some(DisconnectReason::DisconnectRequested),
            })
        }
    }

    /// Removes the tracked node from the trusted set.
    pub(crate) fn remove_peer_from_trusted_set(&mut self, peer_id: PeerId) {
        let Entry::Occupied(mut entry) = self.peers.entry(peer_id) else { return };
        if !entry.get().is_trusted() {
            return
        }

        let peer = entry.get_mut();

        peer.kind = PeerKind::Basic;
    }

    /// Returns the idle peer with the highest reputation.
    ///
    /// Peers that are `trusted`, see [PeerKind], are prioritized as long as they're not currently
    /// marked as banned or backed off.
    ///
    /// If `connect_trusted_nodes_only` is enabled, see [PeersConfig], then this will only consider
    /// `trusted` peers.
    ///
    /// Returns `None` if no peer is available.
    fn best_unconnected(&mut self) -> Option<(PeerId, &mut Peer)> {
        let mut unconnected = self.peers.iter_mut().filter(|(_, peer)| {
            peer.state.is_unconnected() &&
                !peer.is_banned() &&
                !peer.is_backed_off() &&
                (!self.connect_trusted_nodes_only || peer.is_trusted())
        });

        // keep track of the best peer, if there's one
        let mut best_peer = unconnected.next()?;

        if best_peer.1.is_trusted() {
            return Some((*best_peer.0, best_peer.1))
        }

        for maybe_better in unconnected {
            // if the peer is trusted, return it immediately
            if maybe_better.1.is_trusted() {
                return Some((*maybe_better.0, maybe_better.1))
            }

            // otherwise we keep track of the best peer using the reputation
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
        self.tick();

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
    /// [`PeersManager`] is polled.
    pub fn poll(&mut self, cx: &mut Context<'_>) -> Poll<PeerAction> {
        loop {
            // drain buffered actions
            if let Some(action) = self.queued_actions.pop_front() {
                return Poll::Ready(action)
            }

            while let Poll::Ready(Some(cmd)) = self.handle_rx.poll_next_unpin(cx) {
                match cmd {
                    PeerCommand::Add(peer_id, addr) => {
                        self.add_peer(peer_id, addr, None);
                    }
                    PeerCommand::Remove(peer) => self.remove_peer(peer),
                    PeerCommand::ReputationChange(peer_id, rep) => {
                        self.apply_reputation_change(&peer_id, rep)
                    }
                    PeerCommand::GetPeer(peer, tx) => {
                        let _ = tx.send(self.peers.get(&peer).cloned());
                    }
                    PeerCommand::GetPeers(tx) => {
                        let _ = tx.send(self.iter_peers().collect());
                    }
                }
            }

            if self.release_interval.poll_tick(cx).is_ready() {
                let now = std::time::Instant::now();
                let (_, unbanned_peers) = self.ban_list.evict(now);

                for peer_id in unbanned_peers {
                    if let Some(peer) = self.peers.get_mut(&peer_id) {
                        peer.unban();
                    } else {
                        continue
                    }
                    self.queued_actions.push_back(PeerAction::UnBanPeer { peer_id });
                }

                // clear the backoff list of expired backoffs, and mark the relevant peers as
                // ready to be dialed
                self.backed_off_peers.retain(|peer_id, until| {
                    if now > *until {
                        if let Some(peer) = self.peers.get_mut(peer_id) {
                            peer.backed_off = false;
                        }
                        return false
                    }
                    true
                })
            }

            if self.refill_slots_interval.poll_tick(cx).is_ready() {
                // this ensures the manager will be polled periodically, see [Interval::poll_tick]
                let _ = self.refill_slots_interval.poll_tick(cx);
                self.fill_outbound_slots();
            }

            if self.queued_actions.is_empty() {
                return Poll::Pending
            }
        }
    }
}

impl Default for PeersManager {
    fn default() -> Self {
        PeersManager::new(Default::default())
    }
}

/// Tracks stats about connected nodes
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ConnectionInfo {
    /// Counter for currently occupied slots for active outbound connections.
    #[cfg_attr(feature = "serde", serde(skip))]
    num_outbound: usize,
    /// Counter for currently occupied slots for active inbound connections.
    #[cfg_attr(feature = "serde", serde(skip))]
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
        ConnectionInfo {
            num_outbound: 0,
            num_inbound: 0,
            max_outbound: DEFAULT_MAX_PEERS_OUTBOUND,
            max_inbound: DEFAULT_MAX_PEERS_INBOUND,
        }
    }
}

/// Tracks info about a single peer.
#[derive(Debug, Clone)]
pub struct Peer {
    /// Where to reach the peer
    addr: SocketAddr,
    /// Reputation of the peer.
    reputation: i32,
    /// The state of the connection, if any.
    state: PeerConnectionState,
    /// The [`ForkId`] that the peer announced via discovery.
    fork_id: Option<ForkId>,
    /// Whether the entry should be removed after an existing session was terminated.
    remove_after_disconnect: bool,
    /// The kind of peer
    kind: PeerKind,
    /// Whether the peer is currently backed off.
    backed_off: bool,
    /// Counts number of times the peer was backed off due to a severe [BackoffKind].
    severe_backoff_counter: u32,
}

// === impl Peer ===

impl Peer {
    fn new(addr: SocketAddr) -> Self {
        Self::with_state(addr, Default::default())
    }

    fn trusted(addr: SocketAddr) -> Self {
        Self { kind: PeerKind::Trusted, ..Self::new(addr) }
    }

    fn with_state(addr: SocketAddr, state: PeerConnectionState) -> Self {
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

    fn with_kind(addr: SocketAddr, kind: PeerKind) -> Self {
        Self { kind, ..Self::new(addr) }
    }

    /// Resets the reputation of the peer to the default value. This always returns
    /// [`ReputationChangeOutcome::None`].
    fn reset_reputation(&mut self) -> ReputationChangeOutcome {
        self.reputation = DEFAULT_REPUTATION;

        ReputationChangeOutcome::None
    }

    /// Applies a reputation change to the peer and returns what action should be taken.
    fn apply_reputation(&mut self, reputation: i32) -> ReputationChangeOutcome {
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
    fn is_banned(&self) -> bool {
        is_banned_reputation(self.reputation)
    }

    #[inline]
    fn is_backed_off(&self) -> bool {
        self.backed_off
    }

    /// Unbans the peer by resetting its reputation
    #[inline]
    fn unban(&mut self) {
        self.reputation = DEFAULT_REPUTATION
    }

    /// Returns whether this peer is trusted
    #[inline]
    fn is_trusted(&self) -> bool {
        matches!(self.kind, PeerKind::Trusted)
    }
}

/// Outcomes when a reputation change is applied to a peer
enum ReputationChangeOutcome {
    /// Nothing to do.
    None,
    /// Ban the peer.
    Ban,
    /// Ban and disconnect
    DisconnectAndBan,
    /// Unban the peer
    Unban,
}

/// Represents the kind of connection established to the peer, if any
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
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

    /// Returns true if this is an active incoming connection.
    #[inline]
    fn is_incoming(&self) -> bool {
        matches!(self, PeerConnectionState::In)
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
    /// Get information about a peer
    GetPeer(PeerId, oneshot::Sender<Option<Peer>>),
    /// Get node information on all peers
    GetPeers(oneshot::Sender<Vec<NodeRecord>>),
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
    DiscoveryBanPeerId { peer_id: PeerId, ip_addr: IpAddr },
    /// Ban the IP in discovery.
    DiscoveryBanIp { ip_addr: IpAddr },
    /// Ban the peer temporarily
    BanPeer { peer_id: PeerId },
    /// Unban the peer temporarily
    UnBanPeer { peer_id: PeerId },
    /// Emit peerAdded event
    PeerAdded(PeerId),
    /// Emit peerRemoved event
    PeerRemoved(PeerId),
}

/// Config type for initiating a [`PeersManager`] instance
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct PeersConfig {
    /// How often to recheck free slots for outbound connections.
    #[cfg_attr(feature = "serde", serde(with = "humantime_serde"))]
    pub refill_slots_interval: Duration,
    /// Trusted nodes to connect to.
    pub trusted_nodes: HashSet<NodeRecord>,
    /// Connect to trusted nodes only?
    pub connect_trusted_nodes_only: bool,
    /// Basic nodes to connect to.
    #[cfg_attr(feature = "serde", serde(skip))]
    pub basic_nodes: HashSet<NodeRecord>,
    /// How long to ban bad peers.
    #[cfg_attr(feature = "serde", serde(with = "humantime_serde"))]
    pub ban_duration: Duration,
    /// Restrictions on PeerIds and Ips.
    #[cfg_attr(feature = "serde", serde(skip))]
    pub ban_list: BanList,
    /// Restrictions on connections.
    pub connection_info: ConnectionInfo,
    /// How to weigh reputation changes.
    pub reputation_weights: ReputationChangeWeights,
    /// How long to backoff peers that are we failed to connect to for non-fatal reasons, such as
    /// [`DisconnectReason::TooManyPeers`].
    pub backoff_durations: PeerBackoffDurations,
}

impl Default for PeersConfig {
    fn default() -> Self {
        Self {
            refill_slots_interval: Duration::from_millis(1_000),
            connection_info: Default::default(),
            reputation_weights: Default::default(),
            ban_list: Default::default(),
            // Ban peers for 12h
            ban_duration: Duration::from_secs(60 * 60 * 12),
            backoff_durations: Default::default(),
            trusted_nodes: Default::default(),
            connect_trusted_nodes_only: false,
            basic_nodes: Default::default(),
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

    /// Nodes to always connect to.
    pub fn with_trusted_nodes(mut self, nodes: HashSet<NodeRecord>) -> Self {
        self.trusted_nodes = nodes;
        self
    }

    /// Connect only to trusted nodes.
    pub fn with_connect_trusted_nodes_only(mut self, trusted_only: bool) -> Self {
        self.connect_trusted_nodes_only = trusted_only;
        self
    }

    /// Nodes available at launch.
    pub fn with_basic_nodes(mut self, nodes: HashSet<NodeRecord>) -> Self {
        self.basic_nodes = nodes;
        self
    }

    /// Read from file nodes available at launch. Ignored if None.
    pub fn with_basic_nodes_from_file(
        self,
        optional_file: Option<impl AsRef<Path>>,
    ) -> Result<Self, io::Error> {
        let Some(file_path) = optional_file else { return Ok(self) };
        let reader = match std::fs::File::open(file_path.as_ref()) {
            Ok(file) => io::BufReader::new(file),
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(self),
            Err(e) => Err(e)?,
        };
        info!(target: "net::peers", file = %file_path.as_ref().display(), "Loading saved peers");
        let nodes: HashSet<NodeRecord> = serde_json::from_reader(reader)?;
        Ok(self.with_basic_nodes(nodes))
    }
}

/// The durations to use when a backoff should be applied to a peer.
///
/// See also [`BackoffKind`](BackoffKind).
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PeerBackoffDurations {
    /// Applies to connection problems where there is a chance that they will be resolved after the
    /// short duration.
    #[cfg_attr(feature = "serde", serde(with = "humantime_serde"))]
    pub low: Duration,
    /// Applies to more severe connection problems where there is a lower chance that they will be
    /// resolved.
    #[cfg_attr(feature = "serde", serde(with = "humantime_serde"))]
    pub medium: Duration,
    /// Intended for spammers, or bad peers in general.
    #[cfg_attr(feature = "serde", serde(with = "humantime_serde"))]
    pub high: Duration,
    /// Maximum total backoff duration.
    #[cfg_attr(feature = "serde", serde(with = "humantime_serde"))]
    pub max: Duration,
}

impl PeerBackoffDurations {
    /// Returns the corresponding [`Duration`]
    pub fn backoff(&self, kind: BackoffKind) -> Duration {
        match kind {
            BackoffKind::Low => self.low,
            BackoffKind::Medium => self.medium,
            BackoffKind::High => self.high,
        }
    }

    /// Returns the timestamp until which we should backoff.
    ///
    /// The Backoff duration is capped by the configured maximum backoff duration.
    pub fn backoff_until(&self, kind: BackoffKind, backoff_counter: u32) -> std::time::Instant {
        let backoff_time = self.backoff(kind);
        let backoff_time = backoff_time + backoff_time * backoff_counter;
        let now = std::time::Instant::now();
        now + backoff_time.min(self.max)
    }
}

impl Default for PeerBackoffDurations {
    fn default() -> Self {
        Self {
            low: Duration::from_secs(30),
            // 3min
            medium: Duration::from_secs(60 * 3),
            // 15min
            high: Duration::from_secs(60 * 15),
            // 1h
            max: Duration::from_secs(60 * 60),
        }
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
        error::BackoffKind,
        peers::{
            manager::{ConnectionInfo, PeerBackoffDurations, PeerConnectionState},
            reputation::DEFAULT_REPUTATION,
            PeerAction,
        },
        session::PendingSessionHandshakeError,
        PeersConfig,
    };
    use reth_discv4::NodeRecord;
    use reth_eth_wire::{
        errors::{EthHandshakeError, EthStreamError, P2PHandshakeError, P2PStreamError},
        DisconnectReason,
    };
    use reth_net_common::ban_list::BanList;
    use reth_network_api::ReputationChangeKind;
    use reth_primitives::{PeerId, H512};
    use std::{
        collections::HashSet,
        future::{poll_fn, Future},
        io,
        net::{IpAddr, Ipv4Addr, SocketAddr},
        pin::Pin,
        task::{Context, Poll},
        time::Duration,
    };

    struct PeerActionFuture<'a> {
        peers: &'a mut PeersManager,
    }

    impl<'a> Future for PeerActionFuture<'a> {
        type Output = PeerAction;

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
            self.get_mut().peers.poll(cx)
        }
    }

    macro_rules! event {
        ($peers:expr) => {
            PeerActionFuture { peers: &mut $peers }.await
        };
    }

    #[tokio::test]
    async fn test_insert() {
        let peer = PeerId::random();
        let socket_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 1, 2)), 8008);
        let mut peers = PeersManager::default();
        peers.add_peer(peer, socket_addr, None);

        match event!(peers) {
            PeerAction::PeerAdded(peer_id) => {
                assert_eq!(peer_id, peer);
            }
            _ => unreachable!(),
        }
        match event!(peers) {
            PeerAction::Connect { peer_id, remote_addr } => {
                assert_eq!(peer_id, peer);
                assert_eq!(remote_addr, socket_addr);
            }
            _ => unreachable!(),
        }
    }

    #[tokio::test]
    async fn test_ban() {
        let peer = PeerId::random();
        let socket_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 1, 2)), 8008);
        let mut peers = PeersManager::default();
        peers.ban_peer(peer);
        peers.add_peer(peer, socket_addr, None);

        match event!(peers) {
            PeerAction::BanPeer { peer_id } => {
                assert_eq!(peer_id, peer);
            }
            _ => unreachable!(),
        }

        poll_fn(|cx| {
            assert!(peers.poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
    }

    #[tokio::test]
    async fn test_unban() {
        let peer = PeerId::random();
        let socket_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 1, 2)), 8008);
        let mut peers = PeersManager::default();
        peers.ban_peer(peer);
        peers.add_peer(peer, socket_addr, None);

        match event!(peers) {
            PeerAction::BanPeer { peer_id } => {
                assert_eq!(peer_id, peer);
            }
            _ => unreachable!(),
        }

        poll_fn(|cx| {
            assert!(peers.poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;

        peers.unban_peer(peer);

        match event!(peers) {
            PeerAction::UnBanPeer { peer_id } => {
                assert_eq!(peer_id, peer);
            }
            _ => unreachable!(),
        }

        poll_fn(|cx| {
            assert!(peers.poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
    }

    #[tokio::test]
    async fn test_backoff_on_busy() {
        let peer = PeerId::random();
        let socket_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 1, 2)), 8008);

        let backoff_durations =
            PeerBackoffDurations { low: Duration::from_millis(200), ..Default::default() };
        let config = PeersConfig { backoff_durations, ..Default::default() };
        let mut peers = PeersManager::new(config);
        peers.add_peer(peer, socket_addr, None);

        match event!(peers) {
            PeerAction::PeerAdded(peer_id) => {
                assert_eq!(peer_id, peer);
            }
            _ => unreachable!(),
        }
        match event!(peers) {
            PeerAction::Connect { peer_id, .. } => {
                assert_eq!(peer_id, peer);
            }
            _ => unreachable!(),
        }

        poll_fn(|cx| {
            assert!(peers.poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;

        peers.on_active_session_dropped(
            &socket_addr,
            &peer,
            &EthStreamError::P2PStreamError(P2PStreamError::Disconnected(
                DisconnectReason::TooManyPeers,
            )),
        );

        poll_fn(|cx| {
            assert!(peers.poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;

        assert!(peers.backed_off_peers.contains_key(&peer));
        assert!(peers.peers.get(&peer).unwrap().is_backed_off());

        tokio::time::sleep(backoff_durations.low).await;

        match event!(peers) {
            PeerAction::Connect { peer_id, .. } => {
                assert_eq!(peer_id, peer);
            }
            _ => unreachable!(),
        }

        assert!(!peers.backed_off_peers.contains_key(&peer));
        assert!(!peers.peers.get(&peer).unwrap().is_backed_off());
    }

    #[tokio::test]
    async fn test_backoff_on_no_response() {
        let peer = PeerId::random();
        let socket_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 1, 2)), 8008);

        let backoff_durations = PeerBackoffDurations {
            high: Duration::from_millis(200),
            low: Duration::from_millis(200),
            ..Default::default()
        };
        let config = PeersConfig { backoff_durations, ..Default::default() };
        let mut peers = PeersManager::new(config);
        peers.add_peer(peer, socket_addr, None);

        match event!(peers) {
            PeerAction::PeerAdded(peer_id) => {
                assert_eq!(peer_id, peer);
            }
            _ => unreachable!(),
        }
        match event!(peers) {
            PeerAction::Connect { peer_id, .. } => {
                assert_eq!(peer_id, peer);
            }
            _ => unreachable!(),
        }

        poll_fn(|cx| {
            assert!(peers.poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;

        peers.on_pending_session_dropped(
            &socket_addr,
            &peer,
            &PendingSessionHandshakeError::Eth(EthStreamError::EthHandshakeError(
                EthHandshakeError::NoResponse,
            )),
        );

        poll_fn(|cx| {
            assert!(peers.poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;

        assert!(peers.backed_off_peers.contains_key(&peer));
        assert!(peers.peers.get(&peer).unwrap().is_backed_off());

        tokio::time::sleep(backoff_durations.high).await;

        match event!(peers) {
            PeerAction::Connect { peer_id, .. } => {
                assert_eq!(peer_id, peer);
            }
            _ => unreachable!(),
        }

        assert!(!peers.backed_off_peers.contains_key(&peer));
        assert!(!peers.peers.get(&peer).unwrap().is_backed_off());
    }

    #[tokio::test]
    async fn test_low_backoff() {
        let peer = PeerId::random();
        let socket_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 1, 2)), 8008);
        let config = PeersConfig::default();
        let mut peers = PeersManager::new(config);
        peers.add_peer(peer, socket_addr, None);
        let peer_struct = peers.peers.get_mut(&peer).unwrap();

        let backoff_timestamp = peers
            .backoff_durations
            .backoff_until(BackoffKind::Low, peer_struct.severe_backoff_counter);

        let expected = std::time::Instant::now() + peers.backoff_durations.low;
        assert!(backoff_timestamp <= expected);
    }

    #[tokio::test]
    async fn test_multiple_backoff_calculations() {
        let peer = PeerId::random();
        let socket_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 1, 2)), 8008);
        let config = PeersConfig::default();
        let mut peers = PeersManager::new(config);
        peers.add_peer(peer, socket_addr, None);
        let peer_struct = peers.peers.get_mut(&peer).unwrap();

        // Simulate a peer that was already backed off once
        peer_struct.severe_backoff_counter = 1;

        let now = std::time::Instant::now();

        // Simulate the increment that happens in on_connection_failure
        peer_struct.severe_backoff_counter += 1;
        // Get official backoff time
        let backoff_time = peers
            .backoff_durations
            .backoff_until(BackoffKind::High, peer_struct.severe_backoff_counter);

        // Duration of the backoff should be 2 * 15 minutes = 30 minutes
        let backoff_duration = std::time::Duration::new(30 * 60, 0);

        // We can't use assert_eq! since there is a very small diff in the nano secs
        // Usually it is 1800s != 1799.9999996s
        assert!(backoff_time.duration_since(now) > backoff_duration);
    }

    #[tokio::test]
    async fn test_ban_on_active_drop() {
        let peer = PeerId::random();
        let socket_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 1, 2)), 8008);
        let mut peers = PeersManager::default();
        peers.add_peer(peer, socket_addr, None);

        match event!(peers) {
            PeerAction::PeerAdded(peer_id) => {
                assert_eq!(peer_id, peer);
            }
            _ => unreachable!(),
        }
        match event!(peers) {
            PeerAction::Connect { peer_id, .. } => {
                assert_eq!(peer_id, peer);
            }
            _ => unreachable!(),
        }

        poll_fn(|cx| {
            assert!(peers.poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;

        peers.on_active_session_dropped(
            &socket_addr,
            &peer,
            &EthStreamError::P2PStreamError(P2PStreamError::Disconnected(
                DisconnectReason::UselessPeer,
            )),
        );

        match event!(peers) {
            PeerAction::PeerRemoved(peer_id) => {
                assert_eq!(peer_id, peer);
            }
            _ => unreachable!(),
        }
        match event!(peers) {
            PeerAction::BanPeer { peer_id } => {
                assert_eq!(peer_id, peer);
            }
            _ => unreachable!(),
        }

        poll_fn(|cx| {
            assert!(peers.poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;

        assert!(peers.peers.get(&peer).is_none());
    }

    #[tokio::test]
    async fn test_ban_on_pending_drop() {
        let peer = PeerId::random();
        let socket_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 1, 2)), 8008);
        let mut peers = PeersManager::default();
        peers.add_peer(peer, socket_addr, None);

        match event!(peers) {
            PeerAction::PeerAdded(peer_id) => {
                assert_eq!(peer_id, peer);
            }
            _ => unreachable!(),
        }
        match event!(peers) {
            PeerAction::Connect { peer_id, .. } => {
                assert_eq!(peer_id, peer);
            }
            _ => unreachable!(),
        }

        poll_fn(|cx| {
            assert!(peers.poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;

        peers.on_pending_session_dropped(
            &socket_addr,
            &peer,
            &PendingSessionHandshakeError::Eth(EthStreamError::P2PStreamError(
                P2PStreamError::Disconnected(DisconnectReason::UselessPeer),
            )),
        );

        match event!(peers) {
            PeerAction::PeerRemoved(peer_id) => {
                assert_eq!(peer_id, peer);
            }
            _ => unreachable!(),
        }
        match event!(peers) {
            PeerAction::BanPeer { peer_id } => {
                assert_eq!(peer_id, peer);
            }
            _ => unreachable!(),
        }

        poll_fn(|cx| {
            assert!(peers.poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;

        assert!(peers.peers.get(&peer).is_none());
    }

    #[tokio::test]
    async fn test_internally_closed_incoming() {
        let socket_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 1, 2)), 8008);
        let mut peers = PeersManager::default();

        assert!(peers.on_incoming_pending_session(socket_addr.ip()).is_ok());
        assert_eq!(peers.connection_info.num_inbound, 1);
        peers.on_incoming_pending_session_rejected_internally();
        assert_eq!(peers.connection_info.num_inbound, 0);
    }

    #[tokio::test]
    async fn test_closed_incoming() {
        let socket_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 1, 2)), 8008);
        let mut peers = PeersManager::default();

        assert!(peers.on_incoming_pending_session(socket_addr.ip()).is_ok());
        assert_eq!(peers.connection_info.num_inbound, 1);
        peers.on_incoming_pending_session_gracefully_closed();
        assert_eq!(peers.connection_info.num_inbound, 0);
    }

    #[tokio::test]
    async fn test_dropped_incoming() {
        let socket_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(1, 0, 1, 2)), 8008);
        let ban_duration = Duration::from_millis(500);
        let config = PeersConfig { ban_duration, ..Default::default() };
        let mut peers = PeersManager::new(config);

        assert!(peers.on_incoming_pending_session(socket_addr.ip()).is_ok());
        assert_eq!(peers.connection_info.num_inbound, 1);
        let err = PendingSessionHandshakeError::Eth(EthStreamError::P2PStreamError(
            P2PStreamError::HandshakeError(P2PHandshakeError::Disconnected(
                DisconnectReason::UselessPeer,
            )),
        ));

        peers.on_incoming_pending_session_dropped(socket_addr, &err);
        assert_eq!(peers.connection_info.num_inbound, 0);
        assert!(peers.ban_list.is_banned_ip(&socket_addr.ip()));

        assert!(peers.on_incoming_pending_session(socket_addr.ip()).is_err());

        // unbanned after timeout
        tokio::time::sleep(ban_duration).await;

        poll_fn(|cx| {
            let _ = peers.poll(cx);
            Poll::Ready(())
        })
        .await;

        assert!(!peers.ban_list.is_banned_ip(&socket_addr.ip()));
        assert!(peers.on_incoming_pending_session(socket_addr.ip()).is_ok());
    }

    #[tokio::test]
    async fn test_reputation_change_connected() {
        let peer = PeerId::random();
        let socket_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 1, 2)), 8008);
        let mut peers = PeersManager::default();
        peers.add_peer(peer, socket_addr, None);

        match event!(peers) {
            PeerAction::PeerAdded(peer_id) => {
                assert_eq!(peer_id, peer);
            }
            _ => unreachable!(),
        }
        match event!(peers) {
            PeerAction::Connect { peer_id, remote_addr } => {
                assert_eq!(peer_id, peer);
                assert_eq!(remote_addr, socket_addr);
            }
            _ => unreachable!(),
        }

        let p = peers.peers.get_mut(&peer).unwrap();
        assert_eq!(p.state, PeerConnectionState::Out);

        peers.apply_reputation_change(&peer, ReputationChangeKind::BadProtocol);

        let p = peers.peers.get(&peer).unwrap();
        assert_eq!(p.state, PeerConnectionState::DisconnectingOut);
        assert!(p.is_banned());

        peers.on_active_session_gracefully_closed(peer);

        let p = peers.peers.get(&peer).unwrap();
        assert_eq!(p.state, PeerConnectionState::Idle);
        assert!(p.is_banned());

        match event!(peers) {
            PeerAction::Disconnect { peer_id, .. } => {
                assert_eq!(peer_id, peer);
            }
            _ => unreachable!(),
        }
    }

    #[tokio::test]
    async fn test_reputation_management() {
        let peer = PeerId::random();
        let socket_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 1, 2)), 8008);
        let mut peers = PeersManager::default();
        peers.add_peer(peer, socket_addr, None);
        assert_eq!(peers.get_reputation(&peer), Some(0));

        peers.apply_reputation_change(&peer, ReputationChangeKind::Other(1024));
        assert_eq!(peers.get_reputation(&peer), Some(1024));

        peers.apply_reputation_change(&peer, ReputationChangeKind::Reset);
        assert_eq!(peers.get_reputation(&peer), Some(0));
    }

    #[tokio::test]
    async fn test_remove_discovered_active() {
        let peer = PeerId::random();
        let socket_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 1, 2)), 8008);
        let mut peers = PeersManager::default();
        peers.add_peer(peer, socket_addr, None);

        match event!(peers) {
            PeerAction::PeerAdded(peer_id) => {
                assert_eq!(peer_id, peer);
            }
            _ => unreachable!(),
        }
        match event!(peers) {
            PeerAction::Connect { peer_id, remote_addr } => {
                assert_eq!(peer_id, peer);
                assert_eq!(remote_addr, socket_addr);
            }
            _ => unreachable!(),
        }

        let p = peers.peers.get(&peer).unwrap();
        assert_eq!(p.state, PeerConnectionState::Out);

        peers.remove_peer(peer);

        match event!(peers) {
            PeerAction::PeerRemoved(peer_id) => {
                assert_eq!(peer_id, peer);
            }
            _ => unreachable!(),
        }
        match event!(peers) {
            PeerAction::Disconnect { peer_id, .. } => {
                assert_eq!(peer_id, peer);
            }
            _ => unreachable!(),
        }

        let p = peers.peers.get(&peer).unwrap();
        assert_eq!(p.state, PeerConnectionState::DisconnectingOut);

        peers.add_peer(peer, socket_addr, None);
        let p = peers.peers.get(&peer).unwrap();
        assert_eq!(p.state, PeerConnectionState::DisconnectingOut);

        peers.on_active_session_gracefully_closed(peer);
        assert!(peers.peers.get(&peer).is_none());
    }

    #[tokio::test]
    async fn test_outgoing_connection_error() {
        let peer = PeerId::random();
        let socket_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 1, 2)), 8008);
        let mut peers = PeersManager::default();
        peers.add_peer(peer, socket_addr, None);

        match event!(peers) {
            PeerAction::PeerAdded(peer_id) => {
                assert_eq!(peer_id, peer);
            }
            _ => unreachable!(),
        }
        match event!(peers) {
            PeerAction::Connect { peer_id, remote_addr } => {
                assert_eq!(peer_id, peer);
                assert_eq!(remote_addr, socket_addr);
            }
            _ => unreachable!(),
        }

        let p = peers.peers.get(&peer).unwrap();
        assert_eq!(p.state, PeerConnectionState::Out);

        assert_eq!(peers.num_outbound_connections(), 1);

        peers.on_outgoing_connection_failure(
            &socket_addr,
            &peer,
            &io::Error::new(io::ErrorKind::ConnectionRefused, ""),
        );

        assert_eq!(peers.num_outbound_connections(), 0);
    }

    #[tokio::test]
    async fn test_discovery_ban_list() {
        let ip = IpAddr::V4(Ipv4Addr::new(127, 0, 1, 2));
        let socket_addr = SocketAddr::new(ip, 8008);
        let ban_list = BanList::new(HashSet::new(), vec![ip]);
        let config = PeersConfig::default().with_ban_list(ban_list);
        let mut peer_manager = PeersManager::new(config);
        peer_manager.add_peer(H512::default(), socket_addr, None);

        assert!(peer_manager.peers.is_empty());
    }

    #[tokio::test]
    async fn test_on_pending_ban_list() {
        let ip = IpAddr::V4(Ipv4Addr::new(127, 0, 1, 2));
        let socket_addr = SocketAddr::new(ip, 8008);
        let ban_list = BanList::new(HashSet::new(), vec![ip]);
        let config = PeersConfig::default().with_ban_list(ban_list);
        let mut peer_manager = PeersManager::new(config);
        let a = peer_manager.on_incoming_pending_session(socket_addr.ip());
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
        peer_manager.on_incoming_session_established(given_peer_id, socket_addr);

        let Some(PeerAction::DisconnectBannedIncoming { peer_id }) =
            peer_manager.queued_actions.pop_front()
        else {
            panic!()
        };

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

    #[tokio::test]
    async fn test_trusted_peers_are_prioritized() {
        let trusted_peer = PeerId::random();
        let trusted_sock = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 1, 2)), 8008);
        let config = PeersConfig::default().with_trusted_nodes(HashSet::from([NodeRecord {
            address: IpAddr::V4(Ipv4Addr::new(127, 0, 1, 2)),
            tcp_port: 8008,
            udp_port: 8008,
            id: trusted_peer,
        }]));
        let mut peers = PeersManager::new(config);

        let basic_peer = PeerId::random();
        let basic_sock = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 1, 2)), 8009);
        peers.add_peer(basic_peer, basic_sock, None);

        match event!(peers) {
            PeerAction::PeerAdded(peer_id) => {
                assert_eq!(peer_id, basic_peer);
            }
            _ => unreachable!(),
        }
        match event!(peers) {
            PeerAction::Connect { peer_id, remote_addr } => {
                assert_eq!(peer_id, trusted_peer);
                assert_eq!(remote_addr, trusted_sock);
            }
            _ => unreachable!(),
        }
        match event!(peers) {
            PeerAction::Connect { peer_id, remote_addr } => {
                assert_eq!(peer_id, basic_peer);
                assert_eq!(remote_addr, basic_sock);
            }
            _ => unreachable!(),
        }
    }

    #[tokio::test]
    async fn test_connect_trusted_nodes_only() {
        let trusted_peer = PeerId::random();
        let trusted_sock = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 1, 2)), 8008);
        let config = PeersConfig::default()
            .with_trusted_nodes(HashSet::from([NodeRecord {
                address: IpAddr::V4(Ipv4Addr::new(127, 0, 1, 2)),
                tcp_port: 8008,
                udp_port: 8008,
                id: trusted_peer,
            }]))
            .with_connect_trusted_nodes_only(true);
        let mut peers = PeersManager::new(config);

        let basic_peer = PeerId::random();
        let basic_sock = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 1, 2)), 8009);
        peers.add_peer(basic_peer, basic_sock, None);

        match event!(peers) {
            PeerAction::PeerAdded(peer_id) => {
                assert_eq!(peer_id, basic_peer);
            }
            _ => unreachable!(),
        }
        match event!(peers) {
            PeerAction::Connect { peer_id, remote_addr } => {
                assert_eq!(peer_id, trusted_peer);
                assert_eq!(remote_addr, trusted_sock);
            }
            _ => unreachable!(),
        }
        poll_fn(|cx| {
            assert!(peers.poll(cx).is_pending());
            Poll::Ready(())
        })
        .await;
    }

    #[tokio::test]
    async fn test_tick() {
        let ip = IpAddr::V4(Ipv4Addr::new(127, 0, 1, 2));
        let socket_addr = SocketAddr::new(ip, 8008);
        let config = PeersConfig::default();
        let mut peer_manager = PeersManager::new(config);
        let peer_id = PeerId::random();
        peer_manager.add_peer(peer_id, socket_addr, None);

        tokio::time::sleep(Duration::from_secs(1)).await;
        peer_manager.tick();

        // still unconnected
        assert_eq!(peer_manager.peers.get_mut(&peer_id).unwrap().reputation, DEFAULT_REPUTATION);

        // mark as connected
        peer_manager.peers.get_mut(&peer_id).unwrap().state = PeerConnectionState::Out;

        tokio::time::sleep(Duration::from_secs(1)).await;
        peer_manager.tick();

        // still at default reputation
        assert_eq!(peer_manager.peers.get_mut(&peer_id).unwrap().reputation, DEFAULT_REPUTATION);

        peer_manager.peers.get_mut(&peer_id).unwrap().reputation -= 1;

        tokio::time::sleep(Duration::from_secs(1)).await;
        peer_manager.tick();

        // tick applied
        assert!(peer_manager.peers.get_mut(&peer_id).unwrap().reputation >= DEFAULT_REPUTATION);
    }

    #[tokio::test]
    async fn test_remove_incoming_after_disconnect() {
        let peer_id = PeerId::random();
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 1, 2)), 8009);
        let mut peers = PeersManager::default();

        peers.on_incoming_pending_session(addr.ip()).unwrap();
        peers.on_incoming_session_established(peer_id, addr);
        let peer = peers.peers.get(&peer_id).unwrap();
        assert_eq!(peer.state, PeerConnectionState::In);
        assert!(peer.remove_after_disconnect);

        peers.on_active_session_gracefully_closed(peer_id);
        assert!(peers.peers.get(&peer_id).is_none())
    }

    #[tokio::test]
    async fn test_keep_incoming_after_disconnect_if_discovered() {
        let peer_id = PeerId::random();
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 1, 2)), 8009);
        let mut peers = PeersManager::default();

        peers.on_incoming_pending_session(addr.ip()).unwrap();
        peers.on_incoming_session_established(peer_id, addr);
        let peer = peers.peers.get(&peer_id).unwrap();
        assert_eq!(peer.state, PeerConnectionState::In);
        assert!(peer.remove_after_disconnect);

        // trigger discovery manually while the peer is still connected
        peers.add_peer(peer_id, addr, None);

        peers.on_active_session_gracefully_closed(peer_id);

        let peer = peers.peers.get(&peer_id).unwrap();
        assert_eq!(peer.state, PeerConnectionState::Idle);
        assert!(!peer.remove_after_disconnect);
    }
}

use crate::{
    listener::{ConnectionListener, ListenerEvent},
    message::PeerMessage,
    peers::InboundConnectionError,
    protocol::IntoRlpxSubProtocol,
    session::{Direction, PendingSessionHandshakeError, SessionEvent, SessionId, SessionManager},
    state::{NetworkState, StateAction},
};
use futures::Stream;
use reth_eth_wire::{
    capability::CapabilityMessage, errors::EthStreamError, Capabilities, DisconnectReason,
    EthNetworkPrimitives, EthVersion, NetworkPrimitives, Status,
};
use reth_network_api::{PeerRequest, PeerRequestSender};
use reth_network_peers::PeerId;
use std::{
    io,
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tracing::trace;

#[cfg_attr(doc, aquamarine::aquamarine)]
/// Contains the connectivity related state of the network.
///
/// A swarm emits [`SwarmEvent`]s when polled.
///
/// It manages the [`ConnectionListener`] and delegates new incoming connections to the
/// [`SessionManager`]. Outgoing connections are either initiated on demand or triggered by the
/// [`NetworkState`] and also delegated to the [`NetworkState`].
///
/// Following diagram displays the dataflow contained in the [`Swarm`]
///
/// The [`ConnectionListener`] yields incoming [`TcpStream`]s from peers that are spawned as session
/// tasks. After a successful `RLPx` authentication, the task is ready to accept ETH requests or
/// broadcast messages. A task listens for messages from the [`SessionManager`] which include
/// broadcast messages like `Transactions` or internal commands, for example to disconnect the
/// session.
///
/// The [`NetworkState`] keeps track of all connected and discovered peers and can initiate outgoing
/// connections. For each active session, the [`NetworkState`] keeps a sender half of the ETH
/// request channel for the created session and sends requests it receives from the
/// [`StateFetcher`], which receives request objects from the client interfaces responsible for
/// downloading headers and bodies.
///
/// `include_mmd!("docs/mermaid/swarm.mmd`")
#[derive(Debug)]
#[must_use = "Swarm does nothing unless polled"]
pub(crate) struct Swarm<N: NetworkPrimitives = EthNetworkPrimitives> {
    /// Listens for new incoming connections.
    incoming: ConnectionListener,
    /// All sessions.
    sessions: SessionManager<N>,
    /// Tracks the entire state of the network and handles events received from the sessions.
    state: NetworkState<N>,
}

// === impl Swarm ===

impl<N: NetworkPrimitives> Swarm<N> {
    /// Configures a new swarm instance.
    pub(crate) const fn new(
        incoming: ConnectionListener,
        sessions: SessionManager<N>,
        state: NetworkState<N>,
    ) -> Self {
        Self { incoming, sessions, state }
    }

    /// Adds a protocol handler to the `RLPx` sub-protocol list.
    pub(crate) fn add_rlpx_sub_protocol(&mut self, protocol: impl IntoRlpxSubProtocol) {
        self.sessions_mut().add_rlpx_sub_protocol(protocol);
    }

    /// Access to the state.
    pub(crate) const fn state(&self) -> &NetworkState<N> {
        &self.state
    }

    /// Mutable access to the state.
    pub(crate) fn state_mut(&mut self) -> &mut NetworkState<N> {
        &mut self.state
    }

    /// Access to the [`ConnectionListener`].
    pub(crate) const fn listener(&self) -> &ConnectionListener {
        &self.incoming
    }

    /// Access to the [`SessionManager`].
    pub(crate) const fn sessions(&self) -> &SessionManager<N> {
        &self.sessions
    }

    /// Mutable access to the [`SessionManager`].
    pub(crate) fn sessions_mut(&mut self) -> &mut SessionManager<N> {
        &mut self.sessions
    }
}

impl<N: NetworkPrimitives> Swarm<N> {
    /// Triggers a new outgoing connection to the given node
    pub(crate) fn dial_outbound(&mut self, remote_addr: SocketAddr, remote_id: PeerId) {
        self.sessions.dial_outbound(remote_addr, remote_id)
    }

    /// Handles a polled [`SessionEvent`]
    ///
    /// This either updates the state or produces a new [`SwarmEvent`] that is bubbled up to the
    /// manager.
    fn on_session_event(&mut self, event: SessionEvent<N>) -> Option<SwarmEvent<N>> {
        match event {
            SessionEvent::SessionEstablished {
                peer_id,
                remote_addr,
                client_version,
                capabilities,
                version,
                status,
                messages,
                direction,
                timeout,
            } => {
                self.state.on_session_activated(
                    peer_id,
                    capabilities.clone(),
                    status.clone(),
                    messages.clone(),
                    timeout,
                );
                Some(SwarmEvent::SessionEstablished {
                    peer_id,
                    remote_addr,
                    client_version,
                    capabilities,
                    version,
                    messages,
                    status,
                    direction,
                })
            }
            SessionEvent::AlreadyConnected { peer_id, remote_addr, direction } => {
                trace!(target: "net", ?peer_id, ?remote_addr, ?direction, "already connected");
                self.state.peers_mut().on_already_connected(direction);
                None
            }
            SessionEvent::ValidMessage { peer_id, message } => {
                Some(SwarmEvent::ValidMessage { peer_id, message })
            }
            SessionEvent::InvalidMessage { peer_id, capabilities, message } => {
                Some(SwarmEvent::InvalidCapabilityMessage { peer_id, capabilities, message })
            }
            SessionEvent::IncomingPendingSessionClosed { remote_addr, error } => {
                Some(SwarmEvent::IncomingPendingSessionClosed { remote_addr, error })
            }
            SessionEvent::OutgoingPendingSessionClosed { remote_addr, peer_id, error } => {
                Some(SwarmEvent::OutgoingPendingSessionClosed { remote_addr, peer_id, error })
            }
            SessionEvent::Disconnected { peer_id, remote_addr } => {
                self.state.on_session_closed(peer_id);
                Some(SwarmEvent::SessionClosed { peer_id, remote_addr, error: None })
            }
            SessionEvent::SessionClosedOnConnectionError { peer_id, remote_addr, error } => {
                self.state.on_session_closed(peer_id);
                Some(SwarmEvent::SessionClosed { peer_id, remote_addr, error: Some(error) })
            }
            SessionEvent::OutgoingConnectionError { remote_addr, peer_id, error } => {
                Some(SwarmEvent::OutgoingConnectionError { peer_id, remote_addr, error })
            }
            SessionEvent::BadMessage { peer_id } => Some(SwarmEvent::BadMessage { peer_id }),
            SessionEvent::ProtocolBreach { peer_id } => {
                Some(SwarmEvent::ProtocolBreach { peer_id })
            }
        }
    }

    /// Callback for events produced by [`ConnectionListener`].
    ///
    /// Depending on the event, this will produce a new [`SwarmEvent`].
    fn on_connection(&mut self, event: ListenerEvent) -> Option<SwarmEvent<N>> {
        match event {
            ListenerEvent::Error(err) => return Some(SwarmEvent::TcpListenerError(err)),
            ListenerEvent::ListenerClosed { local_address: address } => {
                return Some(SwarmEvent::TcpListenerClosed { remote_addr: address })
            }
            ListenerEvent::Incoming { stream, remote_addr } => {
                // Reject incoming connection if node is shutting down.
                if self.is_shutting_down() {
                    return None
                }
                // ensure we can handle an incoming connection from this address
                if let Err(err) =
                    self.state_mut().peers_mut().on_incoming_pending_session(remote_addr.ip())
                {
                    match err {
                        InboundConnectionError::IpBanned => {
                            trace!(target: "net", ?remote_addr, "The incoming ip address is in the ban list");
                        }
                        InboundConnectionError::ExceedsCapacity => {
                            trace!(target: "net", ?remote_addr, "No capacity for incoming connection");
                            self.sessions.try_disconnect_incoming_connection(
                                stream,
                                DisconnectReason::TooManyPeers,
                            );
                        }
                    }
                    return None
                }

                match self.sessions.on_incoming(stream, remote_addr) {
                    Ok(session_id) => {
                        trace!(target: "net", ?remote_addr, "Incoming connection");
                        return Some(SwarmEvent::IncomingTcpConnection { session_id, remote_addr })
                    }
                    Err(err) => {
                        trace!(target: "net", %err, "Incoming connection rejected, capacity already reached.");
                        self.state_mut()
                            .peers_mut()
                            .on_incoming_pending_session_rejected_internally();
                    }
                }
            }
        }
        None
    }

    /// Hook for actions pulled from the state
    fn on_state_action(&mut self, event: StateAction<N>) -> Option<SwarmEvent<N>> {
        match event {
            StateAction::Connect { remote_addr, peer_id } => {
                self.dial_outbound(remote_addr, peer_id);
                return Some(SwarmEvent::OutgoingTcpConnection { remote_addr, peer_id })
            }
            StateAction::Disconnect { peer_id, reason } => {
                self.sessions.disconnect(peer_id, reason);
            }
            StateAction::NewBlock { peer_id, block: msg } => {
                let msg = PeerMessage::NewBlock(msg);
                self.sessions.send_message(&peer_id, msg);
            }
            StateAction::NewBlockHashes { peer_id, hashes } => {
                let msg = PeerMessage::NewBlockHashes(hashes);
                self.sessions.send_message(&peer_id, msg);
            }
            StateAction::PeerAdded(peer_id) => return Some(SwarmEvent::PeerAdded(peer_id)),
            StateAction::PeerRemoved(peer_id) => return Some(SwarmEvent::PeerRemoved(peer_id)),
            StateAction::DiscoveredNode { peer_id, addr, fork_id } => {
                // Don't try to connect to peer if node is shutting down
                if self.is_shutting_down() {
                    return None
                }
                // Insert peer only if no fork id or a valid fork id
                if fork_id.map_or_else(|| true, |f| self.sessions.is_valid_fork_id(f)) {
                    self.state_mut().peers_mut().add_peer(peer_id, addr, fork_id);
                }
            }
            StateAction::DiscoveredEnrForkId { peer_id, fork_id } => {
                if self.sessions.is_valid_fork_id(fork_id) {
                    self.state_mut().peers_mut().set_discovered_fork_id(peer_id, fork_id);
                } else {
                    self.state_mut().peers_mut().remove_peer(peer_id);
                }
            }
        }
        None
    }

    /// Set network connection state to `ShuttingDown`
    pub(crate) fn on_shutdown_requested(&mut self) {
        self.state_mut().peers_mut().on_shutdown();
    }

    /// Checks if the node's network connection state is '`ShuttingDown`'
    #[inline]
    pub(crate) const fn is_shutting_down(&self) -> bool {
        self.state().peers().connection_state().is_shutting_down()
    }

    /// Set network connection state to `Hibernate` or `Active`
    pub(crate) fn on_network_state_change(&mut self, network_state: NetworkConnectionState) {
        self.state_mut().peers_mut().on_network_state_change(network_state);
    }
}

impl<N: NetworkPrimitives> Stream for Swarm<N> {
    type Item = SwarmEvent<N>;

    /// This advances all components.
    ///
    /// Processes, delegates (internal) commands received from the
    /// [`NetworkManager`](crate::NetworkManager), then polls the [`SessionManager`] which
    /// yields messages produced by individual peer sessions that are then handled. Least
    /// priority are incoming connections that are handled and delegated to
    /// the [`SessionManager`] to turn them into a session.
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        // This loop advances the network's state prioritizing local work [NetworkState] over work
        // coming in from the network [SessionManager], [ConnectionListener]
        // Existing connections are prioritized over new __incoming__ connections
        loop {
            while let Poll::Ready(action) = this.state.poll(cx) {
                if let Some(event) = this.on_state_action(action) {
                    return Poll::Ready(Some(event))
                }
            }

            // poll all sessions
            match this.sessions.poll(cx) {
                Poll::Pending => {}
                Poll::Ready(event) => {
                    if let Some(event) = this.on_session_event(event) {
                        return Poll::Ready(Some(event))
                    }
                    continue
                }
            }

            // poll listener for incoming connections
            match Pin::new(&mut this.incoming).poll(cx) {
                Poll::Pending => {}
                Poll::Ready(event) => {
                    if let Some(event) = this.on_connection(event) {
                        return Poll::Ready(Some(event))
                    }
                    continue
                }
            }

            return Poll::Pending
        }
    }
}

/// All events created or delegated by the [`Swarm`] that represents changes to the state of the
/// network.
pub(crate) enum SwarmEvent<N: NetworkPrimitives = EthNetworkPrimitives> {
    /// Events related to the actual network protocol.
    ValidMessage {
        /// The peer that sent the message
        peer_id: PeerId,
        /// Message received from the peer
        message: PeerMessage<N>,
    },
    /// Received a message that does not match the announced capabilities of the peer.
    InvalidCapabilityMessage {
        peer_id: PeerId,
        /// Announced capabilities of the remote peer.
        capabilities: Arc<Capabilities>,
        /// Message received from the peer.
        message: CapabilityMessage<N>,
    },
    /// Received a bad message from the peer.
    BadMessage {
        /// Identifier of the remote peer.
        peer_id: PeerId,
    },
    /// Remote peer is considered in protocol violation
    ProtocolBreach {
        /// Identifier of the remote peer.
        peer_id: PeerId,
    },
    /// The underlying tcp listener closed.
    TcpListenerClosed {
        /// Address of the closed listener.
        remote_addr: SocketAddr,
    },
    /// The underlying tcp listener encountered an error that we bubble up.
    TcpListenerError(io::Error),
    /// Received an incoming tcp connection.
    ///
    /// This represents the first step in the session authentication process. The swarm will
    /// produce subsequent events once the stream has been authenticated, or was rejected.
    IncomingTcpConnection {
        /// The internal session identifier under which this connection is currently tracked.
        session_id: SessionId,
        /// Address of the remote peer.
        remote_addr: SocketAddr,
    },
    /// An outbound connection is initiated.
    OutgoingTcpConnection {
        /// Address of the remote peer.
        peer_id: PeerId,
        remote_addr: SocketAddr,
    },
    SessionEstablished {
        peer_id: PeerId,
        remote_addr: SocketAddr,
        client_version: Arc<str>,
        capabilities: Arc<Capabilities>,
        /// negotiated eth version
        version: EthVersion,
        messages: PeerRequestSender<PeerRequest<N>>,
        status: Arc<Status>,
        direction: Direction,
    },
    SessionClosed {
        peer_id: PeerId,
        remote_addr: SocketAddr,
        /// Whether the session was closed due to an error
        error: Option<EthStreamError>,
    },
    /// Admin rpc: new peer added
    PeerAdded(PeerId),
    /// Admin rpc: peer removed
    PeerRemoved(PeerId),
    /// Closed an incoming pending session during authentication.
    IncomingPendingSessionClosed {
        remote_addr: SocketAddr,
        error: Option<PendingSessionHandshakeError>,
    },
    /// Closed an outgoing pending session during authentication.
    OutgoingPendingSessionClosed {
        remote_addr: SocketAddr,
        peer_id: PeerId,
        error: Option<PendingSessionHandshakeError>,
    },
    /// Failed to establish a tcp stream to the given address/node
    OutgoingConnectionError { remote_addr: SocketAddr, peer_id: PeerId, error: io::Error },
}

/// Represents the state of the connection of the node. If shutting down,
/// new connections won't be established.
/// When in hibernation mode, the node will not initiate new outbound connections. This is
/// beneficial for sync stages that do not require a network connection.
#[derive(Debug, Default)]
pub enum NetworkConnectionState {
    /// Node is active, new outbound connections will be established.
    #[default]
    Active,
    /// Node is shutting down, no new outbound connections will be established.
    ShuttingDown,
    /// Hibernate Network connection, no new outbound connections will be established.
    Hibernate,
}

impl NetworkConnectionState {
    /// Returns true if the node is active.
    pub(crate) const fn is_active(&self) -> bool {
        matches!(self, Self::Active)
    }

    /// Returns true if the node is shutting down.
    pub(crate) const fn is_shutting_down(&self) -> bool {
        matches!(self, Self::ShuttingDown)
    }
}

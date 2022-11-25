use crate::{
    fetch::StatusUpdate,
    listener::{ConnectionListener, ListenerEvent},
    message::{PeerMessage, PeerRequestSender},
    session::{Direction, SessionEvent, SessionId, SessionManager},
    state::{AddSessionError, NetworkState, StateAction},
};
use futures::Stream;
use reth_eth_wire::{
    capability::{Capabilities, CapabilityMessage},
    error::EthStreamError,
    DisconnectReason,
};
use reth_interfaces::provider::BlockProvider;
use reth_primitives::PeerId;
use std::{
    io,
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tracing::warn;

/// Contains the connectivity related state of the network.
///
/// A swarm emits [`SwarmEvent`]s when polled.
///
/// The manages the [`ConnectionListener`] and delegates new incoming connections to the
/// [`SessionsManager`]. Outgoing connections are either initiated on demand or triggered by the
/// [`NetworkState`] and also delegated to the [`NetworkState`].
#[must_use = "Swarm does nothing unless polled"]
pub(crate) struct Swarm<C> {
    /// Listens for new incoming connections.
    incoming: ConnectionListener,
    /// All sessions.
    sessions: SessionManager,
    /// Tracks the entire state of the network and handles events received from the sessions.
    state: NetworkState<C>,
}

// === impl Swarm ===

impl<C> Swarm<C>
where
    C: BlockProvider,
{
    /// Configures a new swarm instance.
    pub(crate) fn new(
        incoming: ConnectionListener,
        sessions: SessionManager,
        state: NetworkState<C>,
    ) -> Self {
        Self { incoming, sessions, state }
    }

    /// Access to the state.
    pub(crate) fn state(&self) -> &NetworkState<C> {
        &self.state
    }

    /// Mutable access to the state.
    pub(crate) fn state_mut(&mut self) -> &mut NetworkState<C> {
        &mut self.state
    }

    /// Access to the [`ConnectionListener`].
    pub(crate) fn listener(&self) -> &ConnectionListener {
        &self.incoming
    }
    /// Mutable access to the [`SessionManager`].
    pub(crate) fn sessions_mut(&mut self) -> &mut SessionManager {
        &mut self.sessions
    }

    /// Triggers a new outgoing connection to the given node
    pub(crate) fn dial_outbound(&mut self, remote_addr: SocketAddr, remote_id: PeerId) {
        self.sessions.dial_outbound(remote_addr, remote_id)
    }

    /// Handles a polled [`SessionEvent`]
    fn on_session_event(&mut self, event: SessionEvent) -> Option<SwarmEvent> {
        match event {
            SessionEvent::SessionEstablished {
                peer_id,
                remote_addr,
                capabilities,
                status,
                messages,
                direction,
            } => match self.state.on_session_activated(
                peer_id,
                capabilities.clone(),
                status,
                messages.clone(),
            ) {
                Ok(_) => Some(SwarmEvent::SessionEstablished {
                    peer_id,
                    remote_addr,
                    capabilities,
                    messages,
                    direction,
                }),
                Err(err) => {
                    match err {
                        AddSessionError::AtCapacity { peer } => {
                            self.sessions.disconnect(peer, Some(DisconnectReason::TooManyPeers));
                        }
                    };
                    self.state.peers_mut().on_disconnected(&peer_id);
                    None
                }
            },
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
        }
    }

    /// Callback for events produced by [`ConnectionListener`].
    ///
    /// Depending on the event, this will produce a new [`SwarmEvent`].
    fn on_connection(&mut self, event: ListenerEvent) -> Option<SwarmEvent> {
        match event {
            ListenerEvent::Error(err) => return Some(SwarmEvent::TcpListenerError(err)),
            ListenerEvent::ListenerClosed { local_address: address } => {
                return Some(SwarmEvent::TcpListenerClosed { remote_addr: address })
            }
            ListenerEvent::Incoming { stream, remote_addr } => {
                match self.sessions.on_incoming(stream, remote_addr) {
                    Ok(session_id) => {
                        return Some(SwarmEvent::IncomingTcpConnection { session_id, remote_addr })
                    }
                    Err(err) => {
                        warn!(?err, "Incoming connection rejected");
                    }
                }
            }
        }
        None
    }

    /// Hook for actions pulled from the state
    fn on_state_action(&mut self, event: StateAction) -> Option<SwarmEvent> {
        match event {
            StateAction::Connect { remote_addr, peer_id } => {
                self.sessions.dial_outbound(remote_addr, peer_id);
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
            StateAction::StatusUpdate(status) => return Some(SwarmEvent::StatusUpdate(status)),
        }
        None
    }
}

impl<C> Stream for Swarm<C>
where
    C: BlockProvider,
{
    type Item = SwarmEvent;

    /// This advances all components.
    ///
    /// Processes, delegates (internal) commands received from the [`NetworkManager`], then polls
    /// the [`SessionManager`] which yields messages produced by individual peer sessions that are
    /// then handled. Least priority are incoming connections that are handled and delegated to
    /// the [`SessionManager`] to turn them into a session.
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

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
pub(crate) enum SwarmEvent {
    /// Received a node status update.
    StatusUpdate(StatusUpdate),
    /// Events related to the actual network protocol.
    ValidMessage {
        /// The peer that sent the message
        peer_id: PeerId,
        /// Message received from the peer
        message: PeerMessage,
    },
    /// Received a message that does not match the announced capabilities of the peer.
    InvalidCapabilityMessage {
        peer_id: PeerId,
        /// Announced capabilities of the remote peer.
        capabilities: Arc<Capabilities>,
        /// Message received from the peer.
        message: CapabilityMessage,
    },
    /// Received a bad message from the peer.
    BadMessage {
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
        remote_addr: SocketAddr,
    },
    SessionEstablished {
        peer_id: PeerId,
        remote_addr: SocketAddr,
        capabilities: Arc<Capabilities>,
        messages: PeerRequestSender,
        direction: Direction,
    },
    SessionClosed {
        peer_id: PeerId,
        remote_addr: SocketAddr,
        /// Whether the session was closed due to an error
        error: Option<EthStreamError>,
    },
    /// Closed an incoming pending session during authentication.
    IncomingPendingSessionClosed { remote_addr: SocketAddr, error: Option<EthStreamError> },
    /// Closed an outgoing pending session during authentication.
    OutgoingPendingSessionClosed {
        remote_addr: SocketAddr,
        peer_id: PeerId,
        error: Option<EthStreamError>,
    },
    /// Failed to establish a tcp stream to the given address/node
    OutgoingConnectionError { remote_addr: SocketAddr, peer_id: PeerId, error: io::Error },
}

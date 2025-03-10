use crate::{
    connection::{RessPeerRequest, RessProtocolConnection},
    NodeType, RessProtocolMessage, RessProtocolProvider,
};
use reth_eth_wire::{
    capability::SharedCapabilities, multiplex::ProtocolConnection, protocol::Protocol,
};
use reth_network::protocol::{ConnectionHandler, OnNotSupported, ProtocolHandler};
use reth_network_api::{test_utils::PeersHandle, Direction, PeerId};
use std::{
    fmt,
    net::SocketAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::*;

/// The events that can be emitted by our custom protocol.
#[derive(Debug)]
pub enum ProtocolEvent {
    /// Connection established.
    Established {
        /// Connection direction.
        direction: Direction,
        /// Peer ID.
        peer_id: PeerId,
        /// Sender part for forwarding commands.
        to_connection: mpsc::UnboundedSender<RessPeerRequest>,
    },
    /// Number of max active connections exceeded. New connection was rejected.
    MaxActiveConnectionsExceeded {
        /// The current number
        num_active: u64,
    },
}

/// Protocol state is an helper struct to store the protocol events.
#[derive(Clone, Debug)]
pub struct ProtocolState {
    /// Protocol event sender.
    pub events_sender: mpsc::UnboundedSender<ProtocolEvent>,
    /// The number of active connections.
    pub active_connections: Arc<AtomicU64>,
}

impl ProtocolState {
    /// Create new protocol state.
    pub fn new(events_sender: mpsc::UnboundedSender<ProtocolEvent>) -> Self {
        Self { events_sender, active_connections: Arc::default() }
    }

    /// Returns the current number of active connections.
    pub fn active_connections(&self) -> u64 {
        self.active_connections.load(Ordering::Relaxed)
    }
}

/// The protocol handler takes care of incoming and outgoing connections.
#[derive(Clone)]
pub struct RessProtocolHandler<P> {
    /// Provider.
    pub provider: P,
    /// Node type.
    pub node_type: NodeType,
    /// Peers handle.
    pub peers_handle: PeersHandle,
    /// The maximum number of active connections.
    pub max_active_connections: u64,
    /// Current state of the protocol.
    pub state: ProtocolState,
}

impl<P> fmt::Debug for RessProtocolHandler<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RessProtocolHandler")
            .field("node_type", &self.node_type)
            .field("peers_handle", &self.peers_handle)
            .field("max_active_connections", &self.max_active_connections)
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

impl<P> ProtocolHandler for RessProtocolHandler<P>
where
    P: RessProtocolProvider + Clone + Unpin + 'static,
{
    type ConnectionHandler = Self;

    fn on_incoming(&self, socket_addr: SocketAddr) -> Option<Self::ConnectionHandler> {
        let num_active = self.state.active_connections();
        if num_active >= self.max_active_connections {
            trace!(
                target: "ress::net",
                num_active, max_connections = self.max_active_connections, %socket_addr,
                "ignoring incoming connection, max active reached"
            );
            let _ = self
                .state
                .events_sender
                .send(ProtocolEvent::MaxActiveConnectionsExceeded { num_active });
            None
        } else {
            Some(self.clone())
        }
    }

    fn on_outgoing(
        &self,
        socket_addr: SocketAddr,
        peer_id: PeerId,
    ) -> Option<Self::ConnectionHandler> {
        let num_active = self.state.active_connections();
        if num_active >= self.max_active_connections {
            trace!(
                target: "ress::net",
                num_active, max_connections = self.max_active_connections, %socket_addr, %peer_id,
                "ignoring outgoing connection, max active reached"
            );
            let _ = self
                .state
                .events_sender
                .send(ProtocolEvent::MaxActiveConnectionsExceeded { num_active });
            None
        } else {
            Some(self.clone())
        }
    }
}

impl<P> ConnectionHandler for RessProtocolHandler<P>
where
    P: RessProtocolProvider + Clone + Unpin + 'static,
{
    type Connection = RessProtocolConnection<P>;

    fn protocol(&self) -> Protocol {
        RessProtocolMessage::protocol()
    }

    fn on_unsupported_by_peer(
        self,
        _supported: &SharedCapabilities,
        _direction: Direction,
        _peer_id: PeerId,
    ) -> OnNotSupported {
        if self.node_type.is_stateful() {
            OnNotSupported::KeepAlive
        } else {
            OnNotSupported::Disconnect
        }
    }

    fn into_connection(
        self,
        direction: Direction,
        peer_id: PeerId,
        conn: ProtocolConnection,
    ) -> Self::Connection {
        let (tx, rx) = mpsc::unbounded_channel();

        // Emit connection established event.
        self.state
            .events_sender
            .send(ProtocolEvent::Established { direction, peer_id, to_connection: tx })
            .ok();

        // Increment the number of active sessions.
        self.state.active_connections.fetch_add(1, Ordering::Relaxed);

        RessProtocolConnection::new(
            self.provider.clone(),
            self.node_type,
            self.peers_handle,
            peer_id,
            conn,
            UnboundedReceiverStream::from(rx),
            self.state.active_connections,
        )
    }
}

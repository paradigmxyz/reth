use crate::{
    connection::{RessPeerRequest, RessProtocolConnection},
    NodeType, RessProtocolMessage, RessProtocolProvider,
};
use reth_eth_wire::{
    capability::SharedCapabilities, multiplex::ProtocolConnection, protocol::Protocol,
};
use reth_network::protocol::{ConnectionHandler, OnNotSupported, ProtocolHandler};
use reth_network_api::{test_utils::PeersHandle, Direction, PeerId};
use std::{fmt, net::SocketAddr};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;

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
}

/// Protocol state is an helper struct to store the protocol events.
#[derive(Clone, Debug)]
pub struct ProtocolState {
    /// Protocol event sender.
    pub events_sender: mpsc::UnboundedSender<ProtocolEvent>,
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
    /// Current state of the protocol.
    pub state: ProtocolState,
}

impl<P> fmt::Debug for RessProtocolHandler<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RessProtocolHandler")
            .field("node_type", &self.node_type)
            .field("peers_handle", &self.peers_handle)
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

impl<P> ProtocolHandler for RessProtocolHandler<P>
where
    P: RessProtocolProvider + Clone + Unpin + 'static,
{
    type ConnectionHandler = Self;

    fn on_incoming(&self, _socket_addr: SocketAddr) -> Option<Self::ConnectionHandler> {
        Some(self.clone())
    }

    fn on_outgoing(
        &self,
        _socket_addr: SocketAddr,
        _peer_id: PeerId,
    ) -> Option<Self::ConnectionHandler> {
        Some(self.clone())
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

        self.state
            .events_sender
            .send(ProtocolEvent::Established { direction, peer_id, to_connection: tx })
            .ok();

        RessProtocolConnection::new(
            self.provider.clone(),
            self.node_type,
            self.peers_handle,
            peer_id,
            conn,
            UnboundedReceiverStream::from(rx),
        )
    }
}

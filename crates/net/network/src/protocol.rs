//! Support for handling additional RLPx-based application-level protocols.
//!
//! See also <https://github.com/ethereum/devp2p/blob/master/README.md>

use derive_more::Deref;
use futures::Stream;
use reth_eth_wire::{
    capability::{SharedCapabilities, SharedCapability},
    multiplex::ProtocolConnection,
    protocol::Protocol,
};
use reth_network_api::Direction;
use reth_primitives::BytesMut;
use reth_rpc_types::PeerId;
use std::{fmt, net::SocketAddr, pin::Pin};

/// A trait that allows to offer additional RLPx-based application-level protocols when establishing
/// a peer-to-peer connection.
pub trait ProtocolHandler: fmt::Debug + Send + Sync + 'static {
    /// The type responsible for negotiating the protocol with the remote.
    type ConnectionHandler: ConnectionHandler;

    /// Invoked when a new incoming connection from the remote is requested
    ///
    /// If protocols for this outgoing should be announced to the remote, return a connection
    /// handler.
    fn on_incoming(&self, socket_addr: SocketAddr) -> Option<Self::ConnectionHandler>;

    /// Invoked when a new outgoing connection to the remote is requested.
    ///
    /// If protocols for this outgoing should be announced to the remote, return a connection
    /// handler.
    fn on_outgoing(
        &self,
        socket_addr: SocketAddr,
        peer_id: PeerId,
    ) -> Option<Self::ConnectionHandler>;
}

/// A trait that allows to authenticate a protocol after the RLPx connection was established.
pub trait ConnectionHandler: Send + Sync + 'static {
    /// The connection that yields messages to send to the remote.
    ///
    /// The connection will be closed when this stream resolves.
    type Connection: Stream<Item = BytesMut> + Send + ReadConnectionMeta + 'static;

    /// Returns the protocol to announce when the p2p connection will be established.
    ///
    /// This will be used to negotiate capability message id offsets with the remote peer.
    fn protocol() -> Protocol;

    /// Invoked when the RLPx connection has been established by the peer does not share the
    /// protocol.
    fn on_unsupported_by_peer(
        self,
        supported: &SharedCapabilities,
        direction: Direction,
        peer_id: PeerId,
    ) -> OnNotSupported;

    /// Invoked when the RLPx connection was established.
    ///
    /// The returned future should resolve when the connection should disconnect.
    fn into_connection(
        self,
        direction: Direction,
        peer_id: PeerId,
        conn: ProtocolConnection,
    ) -> Option<Self::Connection>;
}

/// Read metadata about the connection that is useful for processing its messages.
pub trait ReadConnectionMeta {
    /// Returns the message id offset of this protocol as negotiated with the peer upon
    /// establishment of the underlying p2p connection.
    fn shared_capability(&self) -> SharedCapability;
}

/// What to do when a protocol is not supported by the remote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OnNotSupported {
    /// Proceed with the connection and ignore the protocol.
    #[default]
    KeepAlive,
    /// Disconnect the connection.
    Disconnect,
}

/// A wrapper type for a RLPx sub-protocol.
#[derive(Debug, Deref)]
pub struct RlpxSubProtocol(Box<dyn DynProtocolHandler>);

pub trait IntoRlpxSubProtocol {
    /// Converts the type into a [RlpxSubProtocol].
    fn into_rlpx_sub_protocol(self) -> RlpxSubProtocol;
}

impl<T> IntoRlpxSubProtocol for T
where
    T: ProtocolHandler + Send + Sync + 'static,
{
    fn into_rlpx_sub_protocol(self) -> RlpxSubProtocol {
        RlpxSubProtocol(Box::new(self))
    }
}

impl IntoRlpxSubProtocol for RlpxSubProtocol {
    fn into_rlpx_sub_protocol(self) -> RlpxSubProtocol {
        self
    }
}

/// Additional RLPx-based sub-protocols.
#[derive(Debug, Default, Deref)]
pub struct RlpxSubProtocols {
    /// All extra protocols
    protocols: Vec<RlpxSubProtocol>,
}

impl RlpxSubProtocols {
    /// Creates a new list for extra protocols.
    pub fn new() -> Self {
        RlpxSubProtocols { protocols: Vec::new() }
    }

    /// Adds a new protocol.
    pub fn push(&mut self, protocol: impl IntoRlpxSubProtocol) {
        self.protocols.push(protocol.into_rlpx_sub_protocol());
    }
}

/// Wrapper trait for ease of use of [`ProtocolHandler`] as trait object.
pub trait DynProtocolHandler: fmt::Debug + Send + Sync + 'static {
    /// Returns the handled protocol.
    fn protocol(&self) -> Protocol;

    /// See [`ProtocolHandler`].
    fn on_incoming(&self, socket_addr: SocketAddr) -> Option<Box<dyn DynConnectionHandler>>;

    /// See [`ProtocolHandler`].
    fn on_outgoing(
        &self,
        socket_addr: SocketAddr,
        peer_id: PeerId,
    ) -> Option<Box<dyn DynConnectionHandler>>;
}

impl<T> DynProtocolHandler for T
where
    T: ProtocolHandler,
{
    fn protocol(&self) -> Protocol {
        <<T as ProtocolHandler>::ConnectionHandler as ConnectionHandler>::protocol()
    }

    fn on_incoming(&self, socket_addr: SocketAddr) -> Option<Box<dyn DynConnectionHandler>> {
        T::on_incoming(self, socket_addr)
            .map(|handler| Box::new(handler) as Box<dyn DynConnectionHandler>)
    }

    fn on_outgoing(
        &self,
        socket_addr: SocketAddr,
        peer_id: PeerId,
    ) -> Option<Box<dyn DynConnectionHandler>> {
        T::on_outgoing(self, socket_addr, peer_id)
            .map(|handler| Box::new(handler) as Box<dyn DynConnectionHandler>)
    }
}

/// Wrapper trait for ease of use of [`ConnectionHandler`] as trait object.
pub trait DynConnectionHandler: Send + Sync + 'static {
    /// See [`ConnectionHandler`].
    fn protocol(&self) -> Protocol;

    /// See [`ConnectionHandler`].
    fn on_unsupported_by_peer(
        self,
        supported: &SharedCapabilities,
        direction: Direction,
        peer_id: PeerId,
    ) -> OnNotSupported;

    /// See [`ConnectionHandler`].
    fn into_connection(
        self,
        direction: Direction,
        peer_id: PeerId,
        conn: ProtocolConnection,
    ) -> Option<Pin<Box<dyn Stream<Item = BytesMut> + Send + 'static>>>;
}

impl<T> DynConnectionHandler for T
where
    T: ConnectionHandler,
{
    fn protocol(&self) -> Protocol {
        T::protocol()
    }

    fn on_unsupported_by_peer(
        self,
        supported: &SharedCapabilities,
        direction: Direction,
        peer_id: PeerId,
    ) -> OnNotSupported {
        T::on_unsupported_by_peer(self, supported, direction, peer_id)
    }

    fn into_connection(
        self,
        direction: Direction,
        peer_id: PeerId,
        conn: ProtocolConnection,
    ) -> Option<Pin<Box<dyn Stream<Item = BytesMut> + Send + 'static>>> {
        T::into_connection(self, direction, peer_id, conn)
            .map(|conn| Box::pin(conn) as Pin<Box<dyn Stream<Item = BytesMut> + Send>>)
    }
}

//! Support for handling additional RLPx-based application-level protocols.
//!
//! See also <https://github.com/ethereum/devp2p/blob/master/README.md>

use derive_more::Deref;
use futures::{Sink, Stream};
use reth_eth_wire::{
    capability::{SharedCapabilities, SharedCapability},
    multiplex::ProtocolProxy,
    protocol::Protocol,
    CanDisconnect,
};
use reth_network_api::Direction;
use reth_primitives::{Bytes, BytesMut};
use reth_rpc_types::PeerId;
use std::{error, fmt, io, net::SocketAddr, pin::Pin};

/// A trait that allows to offer additional RLPx-based application-level protocols when establishing
/// a peer-to-peer connection.
pub trait ProtocolHandler: fmt::Debug + Send + Sync + 'static {
    /// The type responsible for negotiating the protocol with the remote.
    type ConnectionHandler: ConnectionHandler;

    /// Invoked when a new incoming connection from the remote is requested
    ///
    /// If protocols for this outgoing should be announced to the remote, return a connection
    /// handler.
    fn on_incoming(&self, socket_addr: SocketAddr) -> Option<Box<Self::ConnectionHandler>>;

    /// Invoked when a new outgoing connection to the remote is requested.
    ///
    /// If protocols for this outgoing should be announced to the remote, return a connection
    /// handler.
    fn on_outgoing(
        &self,
        socket_addr: SocketAddr,
        peer_id: PeerId,
    ) -> Option<Box<Self::ConnectionHandler>>;
}

/// A trait that allows to authenticate a protocol after the RLPx connection was established.
pub trait ConnectionHandler: Send + Sync + 'static {
    /// The connection that yields messages to send to the remote.
    ///
    /// The connection will be closed when this stream resolves.
    type Connection: Connection + ?Sized;

    /// The connection that transfers messages to and from the remote.
    ///
    /// The connection will be closed when this stream resolves.
    type P2PConnection: P2PConnection + ?Sized;

    /// Returns the protocol to announce when the p2p connection will be established.
    ///
    /// This will be used to negotiate capability message id offsets with the remote peer.
    fn protocol(&self) -> Protocol;

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
        conn: Self::P2PConnection,
    ) -> Option<Pin<Box<Self::Connection>>>;
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

/// An established rlpx sub protocol connection as returned by [`ConnectionHandler`].
pub trait Connection<FromConn = dyn fmt::Debug, ToConn = dyn fmt::Debug, E = dyn error::Error>:
    Stream<Item = FromConn> + Sink<Box<ToConn>, Error = E> + Send + 'static
where
    FromConn: fmt::Debug + ?Sized,
    ToConn: fmt::Debug + ?Sized,
    E: error::Error + ?Sized,
{
}

impl<T, FromConn, ToConn, E> Connection<FromConn, ToConn, E> for T
where
    T: Stream<Item = FromConn> + Sink<Box<ToConn>, Error = E> + Send + ?Sized + 'static,
    FromConn: fmt::Debug + ?Sized,
    ToConn: fmt::Debug + ?Sized,
    E: error::Error + ?Sized,
{
}

/// An established connection to the p2p connection. Passed to [`ConnectionHandler`] to
/// establish a rlpx subprotocol [`Connection`].
pub trait P2PConnection<E = io::Error>:
    Stream<Item = BytesMut> + Sink<Bytes, Error = E> + CanDisconnect<Bytes> + P2PMeta + Send + 'static
{
}

impl<T, E> P2PConnection<E> for T where
    T: Stream<Item = BytesMut>
        + Sink<Bytes, Error = E>
        + CanDisconnect<Bytes>
        + P2PMeta
        + Send
        + ?Sized
        + 'static
{
}

/// Read metadata about the connection that is useful for processing its messages.
pub trait P2PMeta {
    /// Returns the message id offset of this protocol as negotiated with the peer upon
    /// establishment of the underlying p2p connection.
    fn shared_capability(&self) -> &SharedCapability;
}

impl P2PMeta for ProtocolProxy {
    fn shared_capability(&self) -> &SharedCapability {
        self.cap()
    }
}

/// Convenience type setting associated type for [`ProtocolHandler`].
pub type DynProtocolHandler = dyn ProtocolHandler<ConnectionHandler = DynConnectionHandler>;

/// Convenience type setting associated types for [`ConnectionHandler`].
pub type DynConnectionHandler =
    dyn ConnectionHandler<Connection = dyn Connection, P2PConnection = ProtocolProxy>;

/// A wrapper type for a RLPx sub-protocol.
#[derive(Debug, Deref)]
pub struct RlpxSubProtocol(Box<DynProtocolHandler>);

/// A helper trait to convert a [ProtocolHandler] into a dynamic type
pub trait IntoRlpxSubProtocol {
    /// Converts the type into a [RlpxSubProtocol].
    fn into_rlpx_sub_protocol(self) -> RlpxSubProtocol;
}

impl<T> IntoRlpxSubProtocol for T
where
    T: ProtocolHandler<ConnectionHandler = DynConnectionHandler> + Send + Sync + 'static,
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

    /// Returns all additional protocol handlers that should be announced to the remote during the
    /// Rlpx handshake on an incoming connection.
    pub(crate) fn on_incoming(&self, socket_addr: SocketAddr) -> RlpxSubProtocolHandlers {
        RlpxSubProtocolHandlers(
            self.protocols
                .iter()
                .filter_map(|protocol| protocol.0.on_incoming(socket_addr))
                .collect(),
        )
    }

    /// Returns all additional protocol handlers that should be announced to the remote during the
    /// Rlpx handshake on an outgoing connection.
    pub(crate) fn on_outgoing(
        &self,
        socket_addr: SocketAddr,
        peer_id: PeerId,
    ) -> RlpxSubProtocolHandlers {
        RlpxSubProtocolHandlers(
            self.protocols
                .iter()
                .filter_map(|protocol| protocol.0.on_outgoing(socket_addr, peer_id))
                .collect(),
        )
    }
}

/// A set of additional RLPx-based sub-protocol connection handlers.
#[derive(Default)]
pub(crate) struct RlpxSubProtocolHandlers(Vec<Box<dyn DynConnectionHandler>>);

impl RlpxSubProtocolHandlers {
    /// Returns all handlers.
    pub(crate) fn into_iter(self) -> impl Iterator<Item = Box<dyn DynConnectionHandler>> {
        self.0.into_iter()
    }
}

impl Deref for RlpxSubProtocolHandlers {
    type Target = Vec<Box<dyn DynConnectionHandler>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for RlpxSubProtocolHandlers {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

//! Support for handling additional RLPx-based application-level protocols.
//!
//! See also <https://github.com/ethereum/devp2p/blob/master/README.md>

use futures::Stream;
use reth_eth_wire::{
    capability::SharedCapabilities, multiplex::ProtocolConnection, protocol::Protocol,
};
use reth_network_api::Direction;
use reth_primitives::BytesMut;
use reth_rpc_types::PeerId;
use std::{
    fmt,
    net::SocketAddr,
    ops::{Deref, DerefMut},
    pin::Pin,
};

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
    type Connection: Stream<Item = BytesMut> + Send + 'static;

    /// Returns the protocol to announce when the RLPx connection will be established.
    ///
    /// This will be negotiated with the remote peer.
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
        conn: ProtocolConnection,
    ) -> Self::Connection;
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
#[derive(Debug)]
pub struct RlpxSubProtocol(Box<dyn DynProtocolHandler>);

/// A helper trait to convert a [ProtocolHandler] into a dynamic type
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
#[derive(Debug, Default)]
pub struct RlpxSubProtocols {
    /// All extra protocols
    protocols: Vec<RlpxSubProtocol>,
}

impl RlpxSubProtocols {
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

pub(crate) trait DynProtocolHandler: fmt::Debug + Send + Sync + 'static {
    fn on_incoming(&self, socket_addr: SocketAddr) -> Option<Box<dyn DynConnectionHandler>>;

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

/// Wrapper trait for internal ease of use.
pub(crate) trait DynConnectionHandler: Send + Sync + 'static {
    fn protocol(&self) -> Protocol;

    fn on_unsupported_by_peer(
        self,
        supported: &SharedCapabilities,
        direction: Direction,
        peer_id: PeerId,
    ) -> OnNotSupported;

    fn into_connection(
        self: Box<Self>,
        direction: Direction,
        peer_id: PeerId,
        conn: ProtocolConnection,
    ) -> Pin<Box<dyn Stream<Item = BytesMut> + Send + 'static>>;
}

impl<T> DynConnectionHandler for T
where
    T: ConnectionHandler,
{
    fn protocol(&self) -> Protocol {
        T::protocol(self)
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
        self: Box<Self>,
        direction: Direction,
        peer_id: PeerId,
        conn: ProtocolConnection,
    ) -> Pin<Box<dyn Stream<Item = BytesMut> + Send + 'static>> {
        Box::pin(T::into_connection(*self, direction, peer_id, conn))
    }
}

//! Support for handling additional RLPx-based application-level protocols.
//!
//! See also <https://github.com/ethereum/devp2p/blob/master/README.md>

use futures::{Stream, StreamExt};
use reth_eth_wire::{capability::SharedCapability, protocol::Protocol};
use reth_network_api::Direction;
use reth_primitives::BytesMut;
use reth_rpc_types::PeerId;
use std::{
    fmt,
    future::Future,
    net::SocketAddr,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::sync::mpsc::UnboundedSender;
use tokio_stream::wrappers::UnboundedReceiverStream;

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
    /// The future that handles the connection
    type Connection: Future<Output = ()> + Send + 'static;

    /// Returns the protocols to announce when the RLPx connection will be established.
    ///
    /// This will be negotiated with the remote peer.
    fn protocol(&self) -> Protocol;

    /// Invoked when the RLPx connection has been established by the peer does not share the
    /// protocol.
    fn on_unsupported_by_peer(
        self,
        supported: &SharedCapability,
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

/// A connection channel to send and receive messages for a specific protocols.
///
/// This is a [Stream] that returns raw bytes of the received messages for this protocol.
#[derive(Debug)]
pub struct ProtocolConnection {
    from_wire: UnboundedReceiverStream<BytesMut>,
    to_wire: UnboundedSender<ProtocolMessage>,
}

impl ProtocolConnection {
    /// Sends a message to the remote.
    ///
    /// Returns an error if the connection has been disconnected.
    pub fn send(&self, msg: BytesMut) {
        self.to_wire.send(ProtocolMessage::Message(msg)).ok();
    }

    /// Disconnects the connection.
    pub fn disconnect(&self) {
        let _ = self.to_wire.send(ProtocolMessage::Disconnect);
    }
}

impl Stream for ProtocolConnection {
    type Item = BytesMut;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.from_wire.poll_next_unpin(cx)
    }
}

/// Messages that can be sent from a protocol connection
#[derive(Debug)]
pub(crate) enum ProtocolMessage {
    /// New message to send to the remote.
    Message(BytesMut),
    /// Disconnect the connection.
    Disconnect,
}

/// Errors that can occur when handling a protocol.
#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    /// custom error message
    #[error("{0}")]
    Message(String),
    /// Ayn other error
    #[error(transparent)]
    Other(Box<dyn std::error::Error + Send + Sync + 'static>),
}

impl ProtocolError {
    /// Creates a new error with the given message.
    pub fn msg(msg: impl fmt::Display) -> Self {
        ProtocolError::Message(msg.to_string())
    }

    /// Wraps the given error in a `ProtocolError`.
    pub fn new(err: impl std::error::Error + Send + Sync + 'static) -> Self {
        ProtocolError::Other(Box::new(err))
    }
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
        supported: &SharedCapability,
        direction: Direction,
        peer_id: PeerId,
    ) -> OnNotSupported;

    fn into_connection(
        self,
        direction: Direction,
        peer_id: PeerId,
        conn: ProtocolConnection,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>>;
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
        supported: &SharedCapability,
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
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
        Box::pin(T::into_connection(self, direction, peer_id, conn))
    }
}

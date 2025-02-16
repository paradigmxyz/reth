use super::PendingSessionEvent;
use crate::{
    session::{HandshakeInfo, SessionInfo},
    TryFromPeerMessage,
};
use derive_more::Debug;
use futures::{Sink, Stream};
use reth_ecies::stream::ECIESStream;
use reth_eth_wire::{
    capability::RawCapabilityMessage,
    errors::{EthStreamError, P2PStreamError},
    message::EthBroadcastMessage,
    DisconnectReason, EthMessage, EthVersion, NetworkPrimitives, P2PStream,
};
use reth_network_api::{message::NetworkMessage, Direction, PeerId};
use std::{fmt, future::Future, pin::Pin};
use tokio::net::TcpStream;

pub(crate) mod eth;

/// A type alias for a future that resolves to a `PendingSessionEvent`.
pub(crate) type ConnectionFut<N, C> =
    Pin<Box<dyn Future<Output = PendingSessionEvent<N, C>> + Send>>;

/// This trait is responsible for handling the protocol negotiation and authentication.
pub trait NetworkProtocolHandler<N: NetworkPrimitives>:
    fmt::Debug + Send + Sync + Default + 'static
{
    /// The type responsible for negotiating the protocol with the remote.
    type ConnectionHandler: ConnectionHandler<N>;

    /// Invoked when a new incoming connection from the remote is requested
    ///
    /// If protocols for this outgoing should be announced to the remote, return a connection
    /// handler.
    fn on_incoming(
        &self,
        stream: TcpStream,
        session_info: SessionInfo,
        handshake_info: HandshakeInfo,
    ) -> ConnectionFut<N, <Self::ConnectionHandler as ConnectionHandler<N>>::Connection> {
        Box::pin(async move {
            Self::ConnectionHandler::into_connection(
                stream,
                session_info,
                handshake_info,
                Direction::Incoming,
            )
            .await
        })
    }

    /// Invoked when a new outgoing connection to the remote is requested.
    ///
    /// If protocols for this outgoing should be announced to the remote, return a connection
    /// handler.
    fn on_outgoing(
        &self,
        stream: TcpStream,
        session_info: SessionInfo,
        handshake_info: HandshakeInfo,
        remote_peer_id: PeerId,
    ) -> ConnectionFut<N, <Self::ConnectionHandler as ConnectionHandler<N>>::Connection> {
        Box::pin(async move {
            Self::ConnectionHandler::into_connection(
                stream,
                session_info,
                handshake_info,
                Direction::Outgoing(remote_peer_id),
            )
            .await
        })
    }
}

/// A trait responsible to handle authentication and initialization of a p2p connection.
pub trait ConnectionHandler<N: NetworkPrimitives>: Send + Sync + 'static {
    /// A connection resolves to a `PendingSessionEvent`.
    type ConnectionFut: Future<Output = PendingSessionEvent<N, Self::Connection>> + Send + 'static;

    type Connection: NetworkStream<N>;

    /// Invoked when a new connection needs to be established, from either an incoming or outgoing
    /// connection.
    fn into_connection(
        stream: TcpStream,
        session_info: SessionInfo,
        handshake_info: HandshakeInfo,
        direction: Direction,
    ) -> Self::ConnectionFut;
}

/// This trait is responsible to abstract the underlying network connection stream.
pub trait NetworkStream<N: NetworkPrimitives>:
    Stream<Item = Result<Self::Message, EthStreamError>> + Debug + Send + Unpin + 'static
{
    /// The network message.
    type Message: NetworkMessage<N> + TryFromPeerMessage<N> + Into<EthMessage<N>>;

    /// Returns the negotiated ETH version.
    fn version(&self) -> EthVersion;

    /// Consumes the connection and returns the inner stream.
    fn into_inner(self) -> P2PStream<ECIESStream<TcpStream>>;

    /// Returns the inner stream.
    fn inner_mut(&mut self) -> &mut P2PStream<ECIESStream<TcpStream>>;

    /// Returns `true` if the connection is about to disconnect.
    fn is_disconnecting(&self) -> bool;

    /// Starts the disconnect process.
    fn start_disconnect(&mut self, reason: DisconnectReason) -> Result<(), P2PStreamError>;

    /// Returns a sink for the connection.
    fn as_sink(&mut self) -> Pin<Box<dyn Sink<Self::Message, Error = EthStreamError> + '_>>;

    /// Starts send broadcast.
    fn start_send_broadcast(&mut self, msg: EthBroadcastMessage<N>) -> Result<(), EthStreamError>;

    /// Starts send raw.
    fn start_send_raw(&mut self, msg: RawCapabilityMessage) -> Result<(), EthStreamError>;
}

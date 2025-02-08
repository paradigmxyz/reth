use super::PendingSessionEvent;
use crate::session::{HandshakeInfo, SessionInfo};
use futures::Stream;
use reth_ecies::stream::ECIESStream;
use reth_eth_wire::{EthVersion, NetworkPrimitives, P2PStream};
use reth_network_api::{Direction, PeerId};
use std::{fmt, future::Future, pin::Pin, sync::Arc};
use tokio::net::TcpStream;

pub(crate) mod eth;

/// A type alias for a future that resolves to a `PendingSessionEvent`.
pub(crate) type ConnectionFut<C> = Pin<Box<dyn Future<Output = PendingSessionEvent<C>> + Send>>;

/// This trait is responsible for handling the protocol negotiation and authentication.
pub trait ProtocolHandler<N: NetworkPrimitives>: fmt::Debug + Send + Sync + 'static {
    /// The type responsible for negotiating the protocol with the remote.
    type ConnectionHandler: ConnectionHandler;

    /// Invoked when a new incoming connection from the remote is requested
    ///
    /// If protocols for this outgoing should be announced to the remote, return a connection
    /// handler.
    fn on_incoming(
        &self,
        stream: TcpStream,
        session_info: SessionInfo,
        handshake_info: HandshakeInfo,
    ) -> ConnectionFut<<Self::ConnectionHandler as ConnectionHandler>::Connection> {
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
    ) -> ConnectionFut<<Self::ConnectionHandler as ConnectionHandler>::Connection> {
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
pub trait ConnectionHandler: Send + Sync + 'static {
    /// A connection resolves to a `PendingSessionEvent`.
    type ConnectionFut: Future<Output = PendingSessionEvent<Self::Connection>> + Send + 'static;

    type Connection: ConnectionStream;

    /// Invoked when a new connection needs to be established, from either an incoming or outgoing
    /// connection.
    fn into_connection(
        stream: TcpStream,
        session_info: SessionInfo,
        handshake_info: HandshakeInfo,
        direction: Direction,
    ) -> Self::ConnectionFut;
}

/// This trait is responsible to abstract the underlying connection stream.
pub trait ConnectionStream: Stream + Send + Unpin + 'static {
    /// Returns the negotiated ETH version.
    fn version(&self) -> EthVersion;

    /// Consumes the connection and returns the inner stream.
    fn into_inner(self) -> P2PStream<ECIESStream<TcpStream>>;
}

/// A dynamically-dispatchable Ethereum protocol handler.
pub trait DynProtocolHandler<N: NetworkPrimitives, C: ConnectionStream>:
    fmt::Debug + Send + Sync + 'static
{
    /// Handles an incoming connection.
    fn on_incoming(
        &self,
        stream: TcpStream,
        session_info: SessionInfo,
        handshake_info: HandshakeInfo,
    ) -> ConnectionFut<C>;

    /// Handles an outgoing connection.
    fn on_outgoing(
        &self,
        stream: TcpStream,
        session_info: SessionInfo,
        handshake_info: HandshakeInfo,
        remote_peer_id: PeerId,
    ) -> ConnectionFut<C>;
}

impl<N: NetworkPrimitives, T>
    DynProtocolHandler<N, <T::ConnectionHandler as ConnectionHandler>::Connection> for T
where
    T: ProtocolHandler<N> + Send + Sync + 'static,
{
    fn on_incoming(
        &self,
        stream: TcpStream,
        session_info: SessionInfo,
        handshake_info: HandshakeInfo,
    ) -> ConnectionFut<<T::ConnectionHandler as ConnectionHandler>::Connection> {
        T::on_incoming(self, stream, session_info, handshake_info)
    }

    fn on_outgoing(
        &self,
        stream: TcpStream,
        session_info: SessionInfo,
        handshake_info: HandshakeInfo,
        remote_peer_id: PeerId,
    ) -> ConnectionFut<<T::ConnectionHandler as ConnectionHandler>::Connection> {
        T::on_outgoing(self, stream, session_info, handshake_info, remote_peer_id)
    }
}

pub trait IntoProtocol<N: NetworkPrimitives, C: ConnectionStream> {
    fn into_protocol(self) -> Arc<dyn DynProtocolHandler<N, C>>;
}

impl<N: NetworkPrimitives, T>
    IntoProtocol<N, <T::ConnectionHandler as ConnectionHandler>::Connection> for T
where
    T: ProtocolHandler<N> + Send + Sync + 'static,
{
    fn into_protocol(
        self,
    ) -> Arc<dyn DynProtocolHandler<N, <T::ConnectionHandler as ConnectionHandler>::Connection>>
    {
        Arc::new(self)
    }
}

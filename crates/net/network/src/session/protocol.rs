use super::{
    pending::{ConnectionFut, SessionInfo},
    PendingSessionEvent,
};
use crate::session::pending::HandshakeInfo;
use reth_eth_wire::NetworkPrimitives;
use reth_network_api::{Direction, PeerId};
use std::{fmt, future::Future};
use tokio::net::TcpStream;

/// A trait responsible for implementing the Ethereum protocol specifications
/// for a TCP stream when establishing a peer-to-peer connection.
pub trait EthProtocolHandler<N: NetworkPrimitives>: fmt::Debug + Send + Sync + 'static {
    /// The type responsible for negotiating the protocol with the remote.
    type ConnectionHandler: ConnectionHandler<N>;

    /// Invoked when a new incoming connection from the remote is requested
    ///
    /// If protocols for this outgoing should be announced to the remote, return a connection
    /// handler.
    async fn on_incoming(
        &self,
        stream: TcpStream,
        session_info: SessionInfo,
        handshake_info: HandshakeInfo,
    ) -> ConnectionFut<N> {
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
    async fn on_outgoing(
        &self,
        stream: TcpStream,
        session_info: SessionInfo,
        handshake_info: HandshakeInfo,
        remote_peer_id: PeerId,
    ) -> ConnectionFut<N> {
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

/// A trait responsible for handling the authentication and initialization  
/// of a protocol after a peer-to-peer connection is established.
pub trait ConnectionHandler<N: NetworkPrimitives>: Send + Sync + 'static {
    /// A connection resolves to a `PendingSessionEvent`.
    type Connection: Future<Output = PendingSessionEvent<N>> + Send + 'static;

    /// Invoked when a new connection needs to be established, from either an incoming or outgoing
    /// connection.
    fn into_connection(
        stream: TcpStream,
        session_info: SessionInfo,
        handshake_info: HandshakeInfo,
        direction: Direction,
    ) -> Self::Connection;
}

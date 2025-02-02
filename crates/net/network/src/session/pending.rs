use super::{protocol::ConnectionHandler, PendingSessionEvent, SessionId};
use crate::{
    protocol::RlpxSubProtocolHandlers, session::get_ecies_stream, PendingSessionHandshakeError,
};
use reth_chainspec::ForkFilter;
use reth_ecies::stream::ECIESStream;
use reth_eth_wire::{
    multiplex::RlpxProtocolMultiplexer, Capabilities, HelloMessageWithProtocols, NetworkPrimitives,
    Status, UnauthedEthStream, UnauthedP2PStream,
};
use reth_network_api::Direction;
use secp256k1::SecretKey;
use std::{future::Future, net::SocketAddr, pin::Pin, sync::Arc};
use tokio::net::TcpStream;

/// A type alias for a future that resolves to a `PendingSessionEvent`.
pub(crate) type ConnectionFut<N> = Pin<Box<dyn Future<Output = PendingSessionEvent<N>> + Send>>;

impl<N: NetworkPrimitives> ConnectionHandler<N> for EthConnection {
    type Connection = ConnectionFut<N>;

    fn into_connection(
        stream: TcpStream,
        session_info: SessionInfo,
        handshake_info: HandshakeInfo,
        direction: Direction,
    ) -> Self::Connection {
        Box::pin(async move {
            let remote_addr = session_info.remote_addr;
            let session_id = session_info.session_id;
            let stream = match get_ecies_stream(stream, session_info.secret_key, direction).await {
                Ok(stream) => stream,
                Err(error) => {
                    return PendingSessionEvent::EciesAuthError {
                        remote_addr,
                        session_id,
                        error,
                        direction,
                    };
                }
            };
            let unauthed = UnauthedP2PStream::new(stream);

            Self::auth_connection(unauthed, session_info, handshake_info, direction).await
        })
    }
}

pub(crate) struct EthConnection;

impl EthConnection {
    pub(crate) fn auth_connection<N: NetworkPrimitives>(
        unauthed_stream: UnauthedP2PStream<ECIESStream<TcpStream>>,
        session_info: SessionInfo,
        mut handshake_info: HandshakeInfo,
        direction: Direction,
    ) -> ConnectionFut<N> {
        Box::pin(async move {
            let hello_msg = &mut handshake_info.hello_msg;
            let mut extra_handlers = handshake_info.extra_handlers;
            extra_handlers.retain(|handler| hello_msg.try_add_protocol(handler.protocol()).is_ok());

            // P2P handshake
            let (p2p_stream, their_hello) = match unauthed_stream.handshake(hello_msg.clone()).await
            {
                Ok(stream_res) => stream_res,
                Err(err) => {
                    return PendingSessionEvent::Disconnected {
                        remote_addr: session_info.remote_addr,
                        session_id: session_info.session_id,
                        direction,
                        error: Some(PendingSessionHandshakeError::Eth(err.into())),
                    };
                }
            };

            // Validate the negotiated eth version
            let eth_version = match p2p_stream.shared_capabilities().eth_version() {
                Ok(version) => version,
                Err(err) => {
                    return PendingSessionEvent::Disconnected {
                        remote_addr: session_info.remote_addr,
                        session_id: session_info.session_id,
                        direction,
                        error: Some(PendingSessionHandshakeError::Eth(err.into())),
                    };
                }
            };

            // Check if we need to install extra protocols via multiplexing
            let (conn, their_status) = if p2p_stream.shared_capabilities().len() == 1 {
                handshake_info.status_msg.set_eth_version(eth_version);
                let eth_unauthed = UnauthedEthStream::new(p2p_stream);
                match eth_unauthed
                    .handshake(handshake_info.status_msg, handshake_info.fork_filter.clone())
                    .await
                {
                    Ok((eth_stream, their_status)) => (eth_stream.into(), their_status),
                    Err(err) => {
                        return PendingSessionEvent::Disconnected {
                            remote_addr: session_info.remote_addr,
                            session_id: session_info.session_id,
                            direction,
                            error: Some(PendingSessionHandshakeError::Eth(err)),
                        };
                    }
                }
            } else {
                let mut multiplex_stream = RlpxProtocolMultiplexer::new(p2p_stream);

                for handler in extra_handlers.into_iter() {
                    let cap = handler.protocol().cap;
                    let remote_peer_id = their_hello.id;
                    multiplex_stream
                        .install_protocol(&cap, move |conn| {
                            handler.into_connection(direction, remote_peer_id, conn)
                        })
                        .ok();
                }

                match multiplex_stream
                    .into_eth_satellite_stream(
                        handshake_info.status_msg,
                        handshake_info.fork_filter,
                    )
                    .await
                {
                    Ok((multiplex_stream, their_status)) => (multiplex_stream.into(), their_status),
                    Err(err) => {
                        return PendingSessionEvent::Disconnected {
                            remote_addr: session_info.remote_addr,
                            session_id: session_info.session_id,
                            direction,
                            error: Some(PendingSessionHandshakeError::Eth(err)),
                        };
                    }
                }
            };

            PendingSessionEvent::Established {
                session_id: session_info.session_id,
                remote_addr: session_info.remote_addr,
                local_addr: session_info.local_addr,
                peer_id: their_hello.id,
                capabilities: Arc::new(Capabilities::from(their_hello.capabilities)),
                status: Arc::new(their_status),
                conn,
                direction,
                client_id: their_hello.client_version,
            }
        })
    }
}

pub(crate) struct SessionInfo {
    session_id: SessionId,
    remote_addr: SocketAddr,
    secret_key: SecretKey,
    local_addr: Option<SocketAddr>,
}

pub(crate) struct HandshakeInfo {
    hello_msg: HelloMessageWithProtocols,
    status_msg: Status,
    fork_filter: ForkFilter,
    extra_handlers: RlpxSubProtocolHandlers,
}

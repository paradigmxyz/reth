use super::{ConnectionFut, PendingSessionEvent};
use crate::{
    get_ecies_stream,
    protocol::{ConnectionHandler, ProtocolHandler},
    session::{HandshakeInfo, SessionInfo},
    EthRlpxConnection, PendingSessionHandshakeError,
};
use reth_eth_wire::{
    multiplex::RlpxProtocolMultiplexer, Capabilities, NetworkPrimitives, UnauthedEthStream,
    UnauthedP2PStream,
};
use reth_network_api::Direction;
use std::{marker::PhantomData, sync::Arc};
use tokio::net::TcpStream;

/// The Ethereum protocol handler.
#[derive(Clone, Debug, Default)]
pub(crate) struct EthProtocol;

impl<N: NetworkPrimitives> ProtocolHandler<N> for EthProtocol {
    type ConnectionHandler = EthConnection;
}

pub(crate) struct EthConnection;

impl<N: NetworkPrimitives> ConnectionHandler<N> for EthConnection {
    type ConnectionFut = ConnectionFut<N, EthRlpxConnection<N>>;
    type Connection = EthRlpxConnection<N>;

    fn into_connection(
        stream: TcpStream,
        session_info: SessionInfo,
        handshake_info: HandshakeInfo,
        direction: Direction,
    ) -> Self::ConnectionFut {
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

            let mut hello_msg = handshake_info.hello_msg;
            let mut extra_handlers = handshake_info.extra_handlers;
            extra_handlers.retain(|handler| hello_msg.try_add_protocol(handler.protocol()).is_ok());

            // P2P handshake
            let (p2p_stream, their_hello) = match unauthed.handshake(hello_msg.clone()).await {
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
                let mut status_msg = handshake_info.status_msg;
                status_msg.set_eth_version(eth_version);
                let eth_unauthed = UnauthedEthStream::new(p2p_stream);
                match eth_unauthed.handshake(status_msg, handshake_info.fork_filter.clone()).await {
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
                _phantom: PhantomData,
            }
        })
    }
}

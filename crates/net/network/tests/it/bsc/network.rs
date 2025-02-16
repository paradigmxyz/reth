use reth_eth_wire::{Capabilities, NetworkPrimitives, UnauthedEthStream, UnauthedP2PStream};
use reth_network::{
    eth_protocol::{ConnectionFut, ConnectionHandler, NetworkProtocolHandler},
    get_ecies_stream, EthRlpxConnection, HandshakeInfo, PendingSessionEvent,
    PendingSessionHandshakeError, SessionInfo,
};
use reth_network_api::Direction;
use std::{marker::PhantomData, sync::Arc};
use tokio::net::TcpStream;
use tracing::info;

/// The Ethereum protocol handler.
#[derive(Clone, Debug, Default)]
pub(crate) struct BscNetworkProtocol;

impl<N: NetworkPrimitives> NetworkProtocolHandler<N> for BscNetworkProtocol {
    type ConnectionHandler = BscConnection;
}

#[derive(Debug)]
pub(crate) struct BscConnection;

impl<N: NetworkPrimitives> ConnectionHandler<N> for BscConnection {
    type ConnectionFut = ConnectionFut<N, EthRlpxConnection<N>>;
    type Connection = EthRlpxConnection<N>;

    fn into_connection(
        stream: TcpStream,
        session_info: SessionInfo,
        handshake_info: HandshakeInfo,
        direction: Direction,
    ) -> Self::ConnectionFut {
        Box::pin(async move {
            info!("BSC into_connection");
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

            let hello_msg = handshake_info.hello_msg;

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
            info!("Connected to, version: {:?}", eth_version);
            // Check if we need to install extra protocols via multiplexing
            let (conn, their_status) = {
                let mut status_msg = handshake_info.status_msg;
                status_msg.set_eth_version(eth_version);
                let eth_unauthed = UnauthedEthStream::new(p2p_stream);
                match eth_unauthed.handshake(status_msg, handshake_info.fork_filter.clone()).await {
                    Ok((eth_stream, their_status)) => (eth_stream.into(), their_status),
                    Err(err) => {
                        info!("Failed to handshake with peer: {:?}", err);
                        return PendingSessionEvent::Disconnected {
                            remote_addr: session_info.remote_addr,
                            session_id: session_info.session_id,
                            direction,
                            error: Some(PendingSessionHandshakeError::Eth(err)),
                        };
                    }
                }
            };

            info!("Handshake successful");

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

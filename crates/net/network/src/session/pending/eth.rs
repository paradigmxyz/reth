use super::{PendingSession, PendingSessionEvent, SessionId};
use crate::{
    protocol::RlpxSubProtocolHandlers, session::get_ecies_stream, PendingSessionHandshakeError,
};
use futures::{future::Either, FutureExt};
use reth_chainspec::ForkFilter;
use reth_ecies::stream::ECIESStream;
use reth_eth_wire::{
    multiplex::RlpxProtocolMultiplexer, Capabilities, HelloMessageWithProtocols, NetworkPrimitives,
    Status, UnauthedEthStream, UnauthedP2PStream,
};
use reth_network_api::{Direction, PeerId};
use secp256k1::SecretKey;
use std::{net::SocketAddr, sync::Arc};
use tokio::{
    net::TcpStream,
    sync::{mpsc, oneshot},
};

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

pub(crate) struct EthPendingSession;

impl EthPendingSession {
    async fn handle_session<N: NetworkPrimitives>(
        &self,
        disconnect_peer: oneshot::Receiver<()>,
        to_events: mpsc::Sender<PendingSessionEvent<N>>,
        stream: TcpStream,
        session_info: SessionInfo,
        handshake_info: HandshakeInfo,
        direction: Direction,
    ) {
        let remote_addr = session_info.remote_addr;
        let session_id = session_info.session_id;

        let stream = match get_ecies_stream(stream, session_info.secret_key, direction).await {
            Ok(stream) => stream,
            Err(error) => {
                let _ = to_events
                    .send(PendingSessionEvent::EciesAuthError {
                        remote_addr,
                        session_id,
                        error,
                        direction,
                    })
                    .await;
                return;
            }
        };

        let unauthed = UnauthedP2PStream::new(stream);
        let auth = self.authenticate(unauthed, session_info, handshake_info, direction).boxed();

        match futures::future::select(disconnect_peer, auth).await {
            Either::Left((_, _)) => {
                let _ = to_events
                    .send(PendingSessionEvent::Disconnected {
                        remote_addr,
                        session_id,
                        direction,
                        error: None,
                    })
                    .await;
            }
            Either::Right((res, _)) => {
                let _ = to_events.send(res).await;
            }
        }
    }

    pub(crate) async fn authenticate<N: NetworkPrimitives>(
        &self,
        unauthed_stream: UnauthedP2PStream<ECIESStream<TcpStream>>,
        session_info: SessionInfo,
        mut handshake_info: HandshakeInfo,
        direction: Direction,
    ) -> PendingSessionEvent<N> {
        // Add extra protocols to the hello message
        let hello_msg = &mut handshake_info.hello_msg;
        let status_msg = &mut handshake_info.status_msg;
        let extra_handlers = handshake_info.extra_handlers.try_add_hello(hello_msg);

        // conduct the p2p handshake and return the authenticated stream
        let (p2p_stream, their_hello) = match unauthed_stream.handshake(hello_msg.clone()).await {
            Ok(stream_res) => stream_res,
            Err(err) => {
                return PendingSessionEvent::Disconnected {
                    remote_addr: session_info.remote_addr,
                    session_id: session_info.session_id,
                    direction,
                    error: Some(PendingSessionHandshakeError::Eth(err.into())),
                }
            }
        };

        // Ensure we negotiated mandatory eth protocol
        let eth_version = match p2p_stream.shared_capabilities().eth_version() {
            Ok(version) => version,
            Err(err) => {
                return PendingSessionEvent::Disconnected {
                    remote_addr: session_info.remote_addr,
                    session_id: session_info.session_id,
                    direction,
                    error: Some(PendingSessionHandshakeError::Eth(err.into())),
                }
            }
        };

        let (conn, their_status) = if p2p_stream.shared_capabilities().len() == 1 {
            // if the hello handshake was successful we can try status handshake
            //
            // Before trying status handshake, set up the version to negotiated shared version
            status_msg.set_eth_version(eth_version);
            let eth_unauthed = UnauthedEthStream::new(p2p_stream);
            let (eth_stream, their_status) =
                match eth_unauthed.handshake(*status_msg, handshake_info.fork_filter.clone()).await
                {
                    Ok(stream_res) => stream_res,
                    Err(err) => {
                        return PendingSessionEvent::Disconnected {
                            remote_addr: session_info.remote_addr,
                            session_id: session_info.session_id,
                            direction,
                            error: Some(PendingSessionHandshakeError::Eth(err)),
                        }
                    }
                };
            (eth_stream.into(), their_status)
        } else {
            // Multiplex the stream with the extra protocols
            let mut multiplex_stream = RlpxProtocolMultiplexer::new(p2p_stream);

            // install additional handlers
            for handler in extra_handlers.into_iter() {
                let cap = handler.protocol().cap;
                let remote_peer_id = their_hello.id;
                multiplex_stream
                    .install_protocol(&cap, move |conn| {
                        handler.into_connection(direction, remote_peer_id, conn)
                    })
                    .ok();
            }

            let (multiplex_stream, their_status) = match multiplex_stream
                .into_eth_satellite_stream(*status_msg, handshake_info.fork_filter.clone())
                .await
            {
                Ok((multiplex_stream, their_status)) => (multiplex_stream, their_status),
                Err(err) => {
                    return PendingSessionEvent::Disconnected {
                        remote_addr: session_info.remote_addr,
                        session_id: session_info.session_id,
                        direction,
                        error: Some(PendingSessionHandshakeError::Eth(err)),
                    }
                }
            };

            (multiplex_stream.into(), their_status)
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
    }
}

impl<N: NetworkPrimitives> PendingSession<N> for EthPendingSession {
    async fn on_incoming(
        &self,
        disconnect_peer: oneshot::Receiver<()>,
        to_events: mpsc::Sender<PendingSessionEvent<N>>,
        stream: TcpStream,
        session_info: SessionInfo,
        handshake_info: HandshakeInfo,
    ) {
        self.handle_session(
            disconnect_peer,
            to_events,
            stream,
            session_info,
            handshake_info,
            Direction::Incoming,
        )
        .await;
    }

    async fn on_outgoing(
        &self,
        disconnect_peer: oneshot::Receiver<()>,
        to_events: mpsc::Sender<PendingSessionEvent<N>>,
        stream: TcpStream,
        session_info: SessionInfo,
        handshake_info: HandshakeInfo,
        peer_id: PeerId,
    ) {
        self.handle_session(
            disconnect_peer,
            to_events,
            stream,
            session_info,
            handshake_info,
            Direction::Outgoing(peer_id),
        )
        .await;
    }
}

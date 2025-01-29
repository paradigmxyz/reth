use super::{PendingSessionEvent, SessionId};
use crate::{protocol::RlpxSubProtocolHandlers, session::get_ecies_stream};
use reth_chainspec::ForkFilter;
use reth_ecies::stream::ECIESStream;
use reth_eth_wire::{HelloMessageWithProtocols, NetworkPrimitives, Status, UnauthedP2PStream};
use reth_network_api::Direction;
use secp256k1::SecretKey;
use std::net::SocketAddr;
use thiserror::Error;
use tokio::{
    net::TcpStream,
    sync::{mpsc, oneshot},
};

#[derive(Debug, Error)]
pub enum PendingSessionErr {
    #[error("ecies auth error")]
    EciesAuthErr,
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

pub(crate) struct PendingSession<N: NetworkPrimitives> {
    session_info: SessionInfo,
    handshake_info: HandshakeInfo,
    unauthed_stream: UnauthedP2PStream<ECIESStream<tokio::net::TcpStream>>,
    disconnect_peer: oneshot::Receiver<()>,
    to_events: mpsc::Sender<PendingSessionEvent<N>>,
}

impl<N: NetworkPrimitives> PendingSession<N> {
    pub(crate) async fn new(
        session_info: SessionInfo,
        handshake_info: HandshakeInfo,
        stream: TcpStream,
        disconnect_peer: oneshot::Receiver<()>,
        to_events: mpsc::Sender<PendingSessionEvent<N>>,
        direction: Direction,
    ) -> Result<Self, PendingSessionErr> {
        let stream = match get_ecies_stream(stream, session_info.secret_key, direction).await {
            Ok(stream) => stream,
            Err(error) => {
                let _ = to_events
                    .send(PendingSessionEvent::EciesAuthError {
                        remote_addr: session_info.remote_addr,
                        session_id: session_info.session_id,
                        error,
                        direction,
                    })
                    .await;
                return Err(PendingSessionErr::EciesAuthErr);
            }
        };

        Ok(Self {
            session_info,
            handshake_info,
            unauthed_stream: UnauthedP2PStream::new(stream),
            disconnect_peer,
            to_events,
        })
    }

    pub(crate) async fn authenticate(&self, direction: Direction) -> PendingSessionEvent<N> {}
}

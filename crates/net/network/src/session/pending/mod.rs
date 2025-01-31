use super::{PendingSessionEvent, SessionId};
use crate::session::pending::eth::HandshakeInfo;
use eth::SessionInfo;
use reth_eth_wire::NetworkPrimitives;
use reth_network_api::PeerId;
use tokio::{
    net::TcpStream,
    sync::{mpsc, oneshot},
};

pub(crate) mod eth;

pub(crate) trait PendingSession<N: NetworkPrimitives> {
    async fn on_incoming(
        &self,
        disconnect_peer: oneshot::Receiver<()>,
        to_events: mpsc::Sender<PendingSessionEvent<N>>,
        stream: TcpStream,
        session_info: SessionInfo,
        handshake_info: HandshakeInfo,
    );

    async fn on_outgoing(
        &self,
        disconnect_peer: oneshot::Receiver<()>,
        to_events: mpsc::Sender<PendingSessionEvent<N>>,
        stream: TcpStream,
        session_info: SessionInfo,
        handshake_info: HandshakeInfo,
        remote_peer_id: PeerId,
    );
}

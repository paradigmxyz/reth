use super::CustomRlpxConnection;
use crate::subprotocol::protocol::{
    event::ProtocolEvent, handler::ProtocolState, proto::CustomRlpxProtoMessage,
};
use reth_eth_wire::{
    capability::SharedCapabilities, multiplex::ProtocolConnection, protocol::Protocol,
};
use reth_network::protocol::{ConnectionHandler, OnNotSupported};
use reth_network_api::{Direction, PeerId};
use tokio::sync::mpsc;
use tokio_stream::wrappers::UnboundedReceiverStream;

/// The connection handler for the custom RLPx protocol.
pub(crate) struct CustomRlpxConnectionHandler {
    pub(crate) state: ProtocolState,
}

impl ConnectionHandler for CustomRlpxConnectionHandler {
    type Connection = CustomRlpxConnection;

    fn protocol(&self) -> Protocol {
        CustomRlpxProtoMessage::protocol()
    }

    fn on_unsupported_by_peer(
        self,
        _supported: &SharedCapabilities,
        _direction: Direction,
        _peer_id: PeerId,
    ) -> OnNotSupported {
        OnNotSupported::KeepAlive
    }

    fn into_connection(
        self,
        direction: Direction,
        peer_id: PeerId,
        conn: ProtocolConnection,
    ) -> Self::Connection {
        let (tx, rx) = mpsc::unbounded_channel();
        self.state
            .events
            .send(ProtocolEvent::Established { direction, peer_id, to_connection: tx })
            .ok();
        CustomRlpxConnection {
            conn,
            initial_ping: direction.is_outgoing().then(CustomRlpxProtoMessage::ping),
            commands: UnboundedReceiverStream::new(rx),
            pending_pong: None,
        }
    }
}

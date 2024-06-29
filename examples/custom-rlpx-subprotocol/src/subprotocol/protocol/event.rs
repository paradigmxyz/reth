use crate::subprotocol::connection::CustomCommand;
use reth_network::Direction;
use reth_network_api::PeerId;
use tokio::sync::mpsc;

/// The events that can be emitted by our custom protocol.
#[derive(Debug)]
pub(crate) enum ProtocolEvent {
    Established {
        #[allow(dead_code)]
        direction: Direction,
        peer_id: PeerId,
        to_connection: mpsc::UnboundedSender<CustomCommand>,
    },
}

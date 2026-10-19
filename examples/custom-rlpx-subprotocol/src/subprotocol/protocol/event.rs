use crate::subprotocol::connection::CustomCommand;
use reth_ethereum::network::{api::PeerId, Direction};
use tokio::sync::mpsc;

/// The events that can be emitted by our custom protocol.
#[derive(Debug)]
pub(crate) enum ProtocolEvent {
    Established {
        #[expect(dead_code)]
        direction: Direction,
        peer_id: PeerId,
        to_connection: mpsc::UnboundedSender<CustomCommand>,
    },
}

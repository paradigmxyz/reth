use super::*;
use reth_primitives_traits::Block;
use reth_tokio_util::EventStream;
use tokio::sync::oneshot;

/// The message that is broadcast to subscribers of the block import channel.
#[derive(Debug, Clone)]
pub struct NewBlockWithPeer<B> {
    /// The peer that sent the block.
    pub peer_id: PeerId,
    /// The block that was received.
    pub block: B,
}

/// Provides a listener for new blocks on the eth wire protocol.
pub trait EthWireBlockListenerProvider {
    /// The network primitives.
    type Block: Block;

    /// Create a new eth wire block listener.
    fn eth_wire_block_listener(
        &self,
    ) -> impl Future<
        Output = Result<EventStream<NewBlockWithPeer<Self::Block>>, oneshot::error::RecvError>,
    > + Send;
}

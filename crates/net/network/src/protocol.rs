//! Support for handling additional RLPx-based application-level protocols.
//!
//! See also <https://github.com/ethereum/devp2p/blob/master/README.md>


use std::future::Future;
use std::net::SocketAddr;
use tokio::sync::mpsc::UnboundedSender;
use tokio_stream::wrappers::UnboundedReceiverStream;
use reth_eth_wire::capability::{Capability, SharedCapability};
use reth_network_api::Direction;
use reth_primitives::BytesMut;
use reth_rpc_types::PeerId;

/// A trait that allows to offer additional RLPx-based application-level protocols when establishing a peer-to-peer connection.
pub trait ProtocolHandler: Send + Sync + 'static {
    /// The type responsible for negotiating the protocol with the remote.
    type ConnectionHandler: ConnectionHandler;

    /// Invoked when a new incoming connection from the remote is requested
    fn on_incoming(&self, socket_addr: SocketAddr) -> Option<Self::ConnectionHandler>;

    /// Invoked when a new outgoing connection to the remote is requested
    fn on_outgoing(&self, socket_addr: SocketAddr, peer_id: PeerId) -> Option<Self::ConnectionHandler>;
}

pub trait ConnectionHandler: Send + Sync + 'static {
    type AuthError: std::error::Error + Send + Sync + 'static;
    type Authentication: Future<Output = Result<(), Self::AuthError>> + Send + 'static;

    /// Returns the capabilities supported by this protocol.
    ///
    /// This will be negotiated with the remote peer.
    fn capabilities(&self) -> Vec<Capability>;

    fn authenticate(self, direction: Direction, peer_id: PeerId);
}

/// A connection channel to send and receive messages for a specific protocols.
#[derive(Debug)]
pub struct SharedProtocols {
    /// Negotiated capabilities.
    shared_capabilities: Vec<SharedCapability>,
    from_wire: UnboundedReceiverStream<BytesMut>,
    to_wire: UnboundedSender<Result<BytesMut, ()>>
}

impl SharedProtocols {

    pub fn shared_capabilities(&self) -> impl IntoIterator<Item = (&str, u8)>  + '_{
        self.shared_capabilities.iter().map(|c| (c.name(), c.version()))
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_boxable_protocol<T: ProtocolHandler>(handler: T) {
        let _ = Box::new(handler);
    }
}

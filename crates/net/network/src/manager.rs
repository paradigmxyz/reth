//! High level network management.
//!
//! The [`Network`] contains the state of the network as a whole. It controls how connections are
//! handled and keeps track of connections to peers.
//!
//! ## Capabilities
//!
//! The network manages peers depending on their announced capabilities via their RLPx sessions. Most importantly the [Ethereum Wire Protocol](https://github.com/ethereum/devp2p/blob/master/caps/eth.md)(`eth`).
//!
//! ## Overview
//!
//! The [`NetworkManager`] is responsible for advancing the state of the `network`. The `network` is
//! made up of peer-to-peer connections between nodes that are available on the same network.
//! Responsible for peer discovery is ethereum's discovery protocol (discv4, discv5). If the address
//! (IP+port) of our node is published via discovery, remote peers can initiate inbound connections
//! to the local node. Once a (tcp) connection is established, both peers start to authenticate a [RLPx session](https://github.com/ethereum/devp2p/blob/master/rlpx.md) via a handshake. If the handshake was successful, both peers announce their capabilities and are now ready to exchange sub-protocol messages via the RLPx session.

use crate::{
    config::NetworkConfig,
    network::{NetworkHandle, NetworkHandleMessage},
    swarm::{ProtocolEvent, Swarm, SwarmEvent},
    NodeId,
};
use futures::{Future, StreamExt};
use reth_interfaces::provider::BlockProvider;
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::{error, trace};

/// Manages the _entire_ state of the network.
///
/// This is an endless [`Future`] that consistently drives the state of the entire network forward.
#[must_use = "The NetworkManager does nothing unless polled"]
pub struct NetworkManager<C> {
    /// The type that manages the actual network part, which includes connections.
    swarm: Swarm<C>,
    /// Underlying network handle that can be shared.
    handle: NetworkHandle,
    /// Receiver half of the command channel set up between this type and the [`NetworkHandle`]
    from_handle_rx: UnboundedReceiverStream<NetworkHandleMessage>,
    /// Handles block imports.
    block_import: (),

    /// Local copy of the `NodeId` of the local node.
    local_node_id: NodeId,
}

// === impl NetworkManager ===

impl<C> NetworkManager<C>
where
    C: BlockProvider,
{
    /// Creates the manager of a new network.
    ///
    /// The [`NetworkManager`] is an endless future that needs to be polled in order to advance the
    /// state of the entire network.

    pub fn new(_config: NetworkConfig<C>) -> Self {
        todo!()
    }

    /// Returns the [`NetworkHandle`] that can be cloned and shared.
    ///
    /// The [`NetworkHandle`] can be used to interact with this [`NetworkManager`]
    pub fn handle(&self) -> &NetworkHandle {
        &self.handle
    }

    /// Handles an event related to RLPx.
    fn on_protocol_event(&mut self, _event: ProtocolEvent) {}
}

impl<C> Future for NetworkManager<C>
where
    C: BlockProvider,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        // process incoming messages from a handle
        loop {
            let _msg = match this.from_handle_rx.poll_next_unpin(cx) {
                Poll::Ready(Some(msg)) => msg,
                Poll::Pending => break,
                Poll::Ready(None) => {
                    // This is only possible if the channel was deliberately closed since we always
                    // have an instance of `NetworkHandle`
                    error!("network message channel closed.");
                    return Poll::Ready(())
                }
            };
            {
                {}
            }
        }

        // advance the swarm
        while let Poll::Ready(Some(event)) = this.swarm.poll_next_unpin(cx) {
            // handle event
            match event {
                SwarmEvent::ProtocolEvent(event) => this.on_protocol_event(event),
                SwarmEvent::TcpListenerClosed { remote_addr } => {
                    trace!(?remote_addr, target = "net", "TCP listener closed.");
                }
                SwarmEvent::TcpListenerError(err) => {
                    trace!(?err, target = "net", "TCP connection error.");
                }
                SwarmEvent::IncomingTcpConnection { remote_addr, .. } => {
                    trace!(?remote_addr, target = "net", "Incoming connection");
                }
                SwarmEvent::OutgoingTcpConnection { remote_addr } => {
                    trace!(?remote_addr, target = "net", "Starting outbound connection.");
                }
                SwarmEvent::SessionEstablished { node_id, remote_addr } => {
                    trace!(?remote_addr, ?node_id, target = "net", "Session established");
                }
                SwarmEvent::SessionClosed { node_id, remote_addr } => {
                    trace!(?remote_addr, ?node_id, target = "net", "Session disconnected");
                }
            }
        }

        todo!()
    }
}

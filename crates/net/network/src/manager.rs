//! High level network management.
//!
//! The [`Network`] contains the state of the network as a whole. It controls how connections are
//! handled and keeps track of connections to peers.

use crate::{
    config::NetworkConfig,
    network::{NetworkHandle, NetworkHandleMessage},
    swarm::Swarm,
    NodeId,
};
use futures::{Future, StreamExt};
use std::{
    pin::Pin,
    task::{Context, Poll},
};

use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::error;

/// Manages the _entire_ state of the network.
///
/// This is an endless [`Future`] that consistently drives the state of the entire network forward.
#[must_use = "The NetworkManager does nothing unless polled"]
pub struct NetworkManager {
    /// The type that manages the actual network part, which includes connections.
    swarm: Swarm,
    /// Underlying network handle that can be shared.
    handle: NetworkHandle,
    /// Receiver half of the command channel set up between this type and the [`NetworkHandle`]
    from_handle_rx: UnboundedReceiverStream<NetworkHandleMessage>,
    /// Handles block imports.
    block_sink: (),

    /// Local copy of the `NodeId` of the local node.
    local_node_id: NodeId,
}

// === impl NetworkManager ===

impl NetworkManager {
    /// Creates the manager of a new network.
    ///
    /// The [`NetworkManager`] is an endless future that needs to be polled in order to advance the
    /// state of the entire network.

    pub fn new(_config: NetworkConfig) -> Self {
        todo!()
    }

    /// Returns the [`NetworkHandle`] that can be cloned and shared.
    ///
    /// The [`NetworkHandle`] can be used to interact with this [`NetworkManager`]
    ///
    /// ```
    /// # use reth_network::NetworkManager;
    /// # async fn f(manager: NetworkManager) {
    ///
    ///  let handle = manager.handle().clone();
    ///  tokio::task::spawn(async move {
    ///    // use the handle to interact with the `manager`
    ///    let _handle = handle;
    /// });
    ///
    /// manager.await;
    /// # }
    /// ```
    pub fn handle(&self) -> &NetworkHandle {
        &self.handle
    }
}

impl Future for NetworkManager {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        // process incoming messages
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
            {}
        }

        // poll the stream
        while let Poll::Ready(Some(_event)) = this.swarm.poll_next_unpin(cx) {
            // handle event
            {}
        }

        todo!()
    }
}

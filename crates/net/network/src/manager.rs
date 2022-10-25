//! High level network management.
//!
//! The [`Network`] contains the state of the network as a whole. It controls how connections are
//! handled and keeps track of connections to peers.

use crate::{
    config::NetworkConfig,
    network::{NetworkHandle, NetworkHandleMessage},
    swarm::Swarm,
};
use futures::Future;
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use tokio::sync::mpsc::UnboundedReceiver;

/// Manages the _entire_ state of the network.
///
/// This is an endless [`Future`] that consistently drives the state of the entire network forward.
#[must_use = "The NetworkManager does nothing unless polled"]
#[pin_project::pin_project]
pub struct NetworkManager {
    /// The type that manages the actual network part, which includes connections.
    #[pin]
    swarm: Swarm,
    /// Underlying network handle that can be shared.
    handle: NetworkHandle,
    /// Receiver half of the command channel set up between this type and the [`NetworkHandle`]
    from_handle_rx: UnboundedReceiver<NetworkHandleMessage>,
}

// === impl NetworkManager ===

impl NetworkManager {
    /// Creates the manager of a new network.
    ///
    /// The [`NetworkManager`] is an endless future that needs to be polled in order to adavance the
    /// state of the entire network.

    pub fn new(_config: NetworkConfig) {
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

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        todo!()
    }
}

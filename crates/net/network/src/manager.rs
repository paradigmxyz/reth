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
    discovery::Discovery,
    error::NetworkError,
    listener::ConnectionListener,
    message::CapabilityMessage,
    network::{NetworkHandle, NetworkHandleMessage},
    peers::PeersManager,
    session::SessionManager,
    state::NetworkState,
    swarm::{Swarm, SwarmEvent},
    NodeId,
};
use futures::{Future, StreamExt};
use parking_lot::Mutex;
use reth_interfaces::provider::BlockProvider;
use std::{
    net::SocketAddr,
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    task::{Context, Poll},
};
use tokio::sync::mpsc;
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
    block_import_sink: (),
    /// The address of this node that listens for incoming connections.
    listener_address: Arc<Mutex<SocketAddr>>,
    /// Tracks the number of active session (connected peers).
    ///
    /// This is updated via internal events and shared via `Arc` with the [`NetworkHandle`]
    /// Updated by the `NetworkWorker` and loaded by the `NetworkService`.
    num_active_peers: Arc<AtomicUsize>,
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
    pub async fn new(config: NetworkConfig<C>) -> Result<Self, NetworkError> {
        let NetworkConfig {
            client,
            secret_key,
            discovery_v4_config,
            discovery_addr,
            listener_addr,
            state_sync,
            peers_config,
            sessions_config,
        } = config;

        let peers_manger = PeersManager::new(peers_config);
        let peers_handle = peers_manger.handle();

        let incoming = ConnectionListener::bind(listener_addr).await?;
        let listener_address = Arc::new(Mutex::new(incoming.local_address()));

        let discovery = Discovery::new(discovery_addr, discovery_v4_config, secret_key).await?;
        let local_node_id = discovery.local_id();

        // TODO this should also need sk for encrypted sessions
        let sessions = SessionManager::new(sessions_config);
        let state = NetworkState::new(client, discovery, state_sync, peers_manger);

        let swarm = Swarm::new(incoming, sessions, state);

        let (to_manager_tx, from_handle_rx) = mpsc::unbounded_channel();

        let num_active_peers = Arc::new(AtomicUsize::new(0));
        let handle = NetworkHandle::new(
            Arc::clone(&num_active_peers),
            Arc::clone(&listener_address),
            to_manager_tx,
            local_node_id,
            peers_handle,
        );

        Ok(Self {
            swarm,
            handle,
            from_handle_rx: UnboundedReceiverStream::new(from_handle_rx),
            block_import_sink: (),
            listener_address,
            num_active_peers,
            local_node_id,
        })
    }

    /// Returns the [`NetworkHandle`] that can be cloned and shared.
    ///
    /// The [`NetworkHandle`] can be used to interact with this [`NetworkManager`]
    pub fn handle(&self) -> &NetworkHandle {
        &self.handle
    }

    /// Handles an event related to RLPx.
    fn on_capability_message(&mut self, _msg: CapabilityMessage) {}
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
                SwarmEvent::CapabilityMessage(msg) => this.on_capability_message(msg),
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
                    let total_active = this.num_active_peers.fetch_add(1, Ordering::Relaxed) + 1;
                    trace!(
                        ?remote_addr,
                        ?node_id,
                        ?total_active,
                        target = "net",
                        "Session established"
                    );
                }
                SwarmEvent::SessionClosed { node_id, remote_addr } => {
                    let total_active = this.num_active_peers.fetch_sub(1, Ordering::Relaxed) - 1;
                    trace!(
                        ?remote_addr,
                        ?node_id,
                        ?total_active,
                        target = "net",
                        "Session disconnected"
                    );
                }
            }
        }

        todo!()
    }
}

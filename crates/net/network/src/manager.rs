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
    network::{NetworkHandle, NetworkHandleMessage},
    peers::PeersManager,
    session::SessionManager,
    state::NetworkState,
    swarm::{Swarm, SwarmEvent},
    NodeId,
};
use futures::{Future, StreamExt};
use parking_lot::Mutex;
use reth_eth_wire::{
    capability::{Capabilities, CapabilityMessage},
    EthMessage,
};
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
///
/// The [`NetworkManager`] is the container type for all parts involved with advancing the network.
#[cfg_attr(doc, aquamarine::aquamarine)]
/// ```mermaid
///  graph TB
///    handle(NetworkHandle)
///    events(NetworkEvents)
///    subgraph NetworkManager
///      direction LR
///      subgraph Swarm
///          direction TB
///          B1[(Peer Sessions)]
///          B2[(Connection Lister)]
///          B3[(State)]
///      end
///    end
///   handle <--> |request/response channel| NetworkManager
///   NetworkManager --> |Network events| events
/// ```
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
    /// All listeners for [`Network`] events.
    event_listeners: NetworkEventListeners,
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
            peers_config,
            sessions_config,
            genesis_hash,
            ..
        } = config;

        let peers_manger = PeersManager::new(peers_config);
        let peers_handle = peers_manger.handle();

        let incoming = ConnectionListener::bind(listener_addr).await?;
        let listener_address = Arc::new(Mutex::new(incoming.local_address()));

        let discovery = Discovery::new(discovery_addr, secret_key, discovery_v4_config).await?;
        // need to retrieve the addr here since provided port could be `0`
        let local_node_id = discovery.local_id();

        let sessions = SessionManager::new(secret_key, sessions_config);
        let state = NetworkState::new(client, discovery, peers_manger, genesis_hash);

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
            event_listeners: Default::default(),
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

    /// Event hook for an unexpected message from the peer.
    fn on_invalid_message(
        &self,
        node_id: NodeId,
        _capabilities: Arc<Capabilities>,
        _message: CapabilityMessage,
    ) {
        trace!(?node_id, target = "net", "received unexpected message");
        // TODO: disconnect?
    }

    /// Handles a received [`CapabilityMessage`] from the peer.
    fn on_capability_message(&mut self, _node_id: NodeId, msg: CapabilityMessage) {
        match msg {
            CapabilityMessage::Eth(eth) => {
                match eth {
                    EthMessage::Status(_) => {}
                    EthMessage::NewBlockHashes(_) => {
                        // update peer's state, to track what blocks this peer has seen
                    }
                    EthMessage::NewBlock(_) => {
                        // emit new block and track that the peer knows this block
                    }
                    EthMessage::Transactions(_) => {
                        // need to emit this as event/send to tx handler
                    }
                    EthMessage::NewPooledTransactionHashes(_) => {
                        // need to emit this as event/send to tx handler
                    }

                    // TODO: should remove the response types here, as they are handled separately
                    EthMessage::GetBlockHeaders(_) => {}
                    EthMessage::BlockHeaders(_) => {}
                    EthMessage::GetBlockBodies(_) => {}
                    EthMessage::BlockBodies(_) => {}
                    EthMessage::GetPooledTransactions(_) => {}
                    EthMessage::PooledTransactions(_) => {}
                    EthMessage::GetNodeData(_) => {}
                    EthMessage::NodeData(_) => {}
                    EthMessage::GetReceipts(_) => {}
                    EthMessage::Receipts(_) => {}
                }
            }
            CapabilityMessage::Other(_) => {
                // other subprotocols
            }
        }
    }

    /// Handler for received messages from a handle
    fn on_handle_message(&mut self, msg: NetworkHandleMessage) {
        match msg {
            NetworkHandleMessage::EventListener(tx) => {
                self.event_listeners.listeners.push(tx);
            }
            NetworkHandleMessage::NewestBlock(_, _) => {}
            _ => {}
        }
    }
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
            match this.from_handle_rx.poll_next_unpin(cx) {
                Poll::Pending => break,
                Poll::Ready(None) => {
                    // This is only possible if the channel was deliberately closed since we always
                    // have an instance of `NetworkHandle`
                    error!("network message channel closed.");
                    return Poll::Ready(())
                }
                Poll::Ready(Some(msg)) => this.on_handle_message(msg),
            };
        }

        // advance the swarm
        while let Poll::Ready(Some(event)) = this.swarm.poll_next_unpin(cx) {
            // handle event
            match event {
                SwarmEvent::CapabilityMessage { node_id, message } => {
                    this.on_capability_message(node_id, message)
                }
                SwarmEvent::InvalidCapabilityMessage { node_id, capabilities, message } => {
                    this.on_invalid_message(node_id, capabilities, message)
                }
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
                SwarmEvent::IncomingPendingSessionClosed { .. } => {}
                SwarmEvent::OutgoingPendingSessionClosed { .. } => {}
                SwarmEvent::OutgoingConnectionError { .. } => {}
            }
        }

        todo!()
    }
}

/// Events emitted by the network that are of interest for subscribers.
#[derive(Debug, Clone)]
pub enum NetworkEvent {
    EthMessage { node_id: NodeId, message: EthMessage },
}

/// Bundles all listeners for [`NetworkEvent`]s.
#[derive(Default)]
struct NetworkEventListeners {
    /// All listeners for an event
    listeners: Vec<mpsc::UnboundedSender<NetworkEvent>>,
}

// === impl NetworkEventListeners ===

impl NetworkEventListeners {
    /// Sends  the event to all listeners.
    ///
    /// Remove channels that got closed.
    fn send(&mut self, event: NetworkEvent) {
        self.listeners.retain(|listener| {
            let open = listener.send(event.clone()).is_ok();
            if !open {
                trace!(target = "net", "event listener channel closed",);
            }
            open
        });
    }
}

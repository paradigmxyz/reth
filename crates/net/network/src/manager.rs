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
    import::{BlockImport, BlockImportOutcome, BlockValidation},
    listener::ConnectionListener,
    message::{NewBlockMessage, PeerMessage, PeerRequest, PeerRequestSender},
    network::{NetworkHandle, NetworkHandleMessage},
    peers::PeersManager,
    session::SessionManager,
    state::NetworkState,
    swarm::{Swarm, SwarmEvent},
    transactions::NetworkTransactionEvent,
};
use futures::{Future, StreamExt};
use parking_lot::Mutex;
use reth_eth_wire::capability::{Capabilities, CapabilityMessage};
use reth_interfaces::provider::BlockProvider;
use reth_primitives::PeerId;
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
    /// Handles block imports according to the `eth` protocol.
    block_import: Box<dyn BlockImport>,
    /// The address of this node that listens for incoming connections.
    listener_address: Arc<Mutex<SocketAddr>>,
    /// All listeners for [`Network`] events.
    event_listeners: NetworkEventListeners,
    /// Sender half to send events to the [`TransactionsManager`] task, if configured.
    to_transactions: Option<mpsc::UnboundedSender<NetworkTransactionEvent>>,
    /// Tracks the number of active session (connected peers).
    ///
    /// This is updated via internal events and shared via `Arc` with the [`NetworkHandle`]
    /// Updated by the `NetworkWorker` and loaded by the `NetworkService`.
    num_active_peers: Arc<AtomicUsize>,
    /// Local copy of the `PeerId` of the local node.
    local_node_id: PeerId,
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
            block_import,
            network_mode,
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
            network_mode,
        );

        Ok(Self {
            swarm,
            handle,
            from_handle_rx: UnboundedReceiverStream::new(from_handle_rx),
            block_import,
            listener_address,
            event_listeners: Default::default(),
            to_transactions: None,
            num_active_peers,
            local_node_id,
        })
    }

    /// Sets the dedicated channel for events indented for the [`TransactionsManager`]
    pub fn set_transactions(&mut self, tx: mpsc::UnboundedSender<NetworkTransactionEvent>) {
        self.to_transactions = Some(tx);
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
        node_id: PeerId,
        _capabilities: Arc<Capabilities>,
        _message: CapabilityMessage,
    ) {
        trace!(?node_id, target = "net", "received unexpected message");
        // TODO: disconnect?
    }

    /// Sends an event to the [`TransactionsManager`] if configured
    fn notify_tx_manager(&self, event: NetworkTransactionEvent) {
        if let Some(ref tx) = self.to_transactions {
            let _ = tx.send(event);
        }
    }

    /// Handle an incoming request from the peer
    fn on_eth_request(&mut self, peer_id: PeerId, req: PeerRequest) {
        match req {
            PeerRequest::GetBlockHeaders { .. } => {}
            PeerRequest::GetBlockBodies { .. } => {}
            PeerRequest::GetPooledTransactions { request, response } => {
                self.notify_tx_manager(NetworkTransactionEvent::GetPooledTransactions {
                    peer_id,
                    request,
                    response,
                });
            }
            PeerRequest::GetNodeData { .. } => {}
            PeerRequest::GetReceipts { .. } => {}
        }
    }

    /// Invoked after a `NewBlock` message from the peer was validated
    fn on_block_import_result(&mut self, outcome: BlockImportOutcome) {
        let BlockImportOutcome { peer, result } = outcome;
        match result {
            Ok(validated_block) => match validated_block {
                BlockValidation::ValidHeader { block } => {
                    self.swarm.state_mut().update_peer_block(&peer, block.hash, block.number());
                    self.swarm.state_mut().announce_new_block(block);
                }
                BlockValidation::ValidBlock { block } => {
                    self.swarm.state_mut().announce_new_block_hash(block);
                }
            },
            Err(_err) => {
                // TODO report peer for bad block
            }
        }
    }

    /// Enforces [EIP-3675](https://eips.ethereum.org/EIPS/eip-3675#devp2p) consensus rules for the network protocol
    ///
    /// Depending on the mode of the network:
    ///    - disconnect peer if in POS
    ///    - execute the closure if in POW
    fn within_pow_or_disconnect<F>(&mut self, peer_id: PeerId, only_pow: F)
    where
        F: FnOnce(&mut Self),
    {
        // reject message in POS
        if self.handle.mode().is_stake() {
            // connections to peers which send invalid messages should be terminated
            self.swarm.sessions_mut().disconnect(peer_id, None);
        } else {
            only_pow(self);
        }
    }

    /// Handles a received Message from the peer.
    fn on_peer_message(&mut self, peer_id: PeerId, msg: PeerMessage) {
        match msg {
            PeerMessage::NewBlockHashes(hashes) => {
                self.within_pow_or_disconnect(peer_id, |this| {
                    let hashes = Arc::try_unwrap(hashes).unwrap_or_else(|arc| (*arc).clone());
                    // update peer's state, to track what blocks this peer has seen
                    this.swarm.state_mut().on_new_block_hashes(peer_id, hashes.0)
                })
            }
            PeerMessage::NewBlock(block) => {
                self.within_pow_or_disconnect(peer_id, move |this| {
                    this.swarm.state_mut().on_new_block(peer_id, block.hash);
                    // start block import process
                    this.block_import.on_new_block(peer_id, block);
                });
            }
            PeerMessage::PooledTransactions(msg) => {
                self.notify_tx_manager(NetworkTransactionEvent::IncomingPooledTransactionHashes {
                    peer_id,
                    msg,
                });
            }
            PeerMessage::Transactions(msg) => {
                self.notify_tx_manager(NetworkTransactionEvent::IncomingTransactions {
                    peer_id,
                    msg,
                });
            }
            PeerMessage::EthRequest(req) => {
                self.on_eth_request(peer_id, req);
            }
            PeerMessage::Other(_) => {}
        }
    }

    /// Handler for received messages from a handle
    fn on_handle_message(&mut self, msg: NetworkHandleMessage) {
        match msg {
            NetworkHandleMessage::EventListener(tx) => {
                self.event_listeners.listeners.push(tx);
            }
            NetworkHandleMessage::AnnounceBlock(block, hash) => {
                if self.handle.mode().is_stake() {
                    error!(target = "net", "Block propagation is not supported in POS - [EIP-3675](https://eips.ethereum.org/EIPS/eip-3675#devp2p)");
                    return
                }
                let msg = NewBlockMessage { hash, block: Arc::new(block) };
                self.swarm.state_mut().announce_new_block(msg);
            }
            NetworkHandleMessage::EthRequest { peer_id, request } => {
                self.swarm.sessions_mut().send_message(&peer_id, PeerMessage::EthRequest(request))
            }
            NetworkHandleMessage::SendTransaction { peer_id, msg } => {
                self.swarm.sessions_mut().send_message(&peer_id, PeerMessage::Transactions(msg))
            }
            NetworkHandleMessage::SendPooledTransactionHashes { peer_id, msg } => self
                .swarm
                .sessions_mut()
                .send_message(&peer_id, PeerMessage::PooledTransactions(msg)),
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

        // poll new block imports
        while let Poll::Ready(outcome) = this.block_import.poll(cx) {
            this.on_block_import_result(outcome);
        }

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
                SwarmEvent::ValidMessage { node_id, message } => {
                    this.on_peer_message(node_id, message)
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
                SwarmEvent::SessionEstablished {
                    node_id: peer_id,
                    remote_addr,
                    capabilities,
                    messages,
                } => {
                    let total_active = this.num_active_peers.fetch_add(1, Ordering::Relaxed) + 1;
                    trace!(
                        ?remote_addr,
                        ?peer_id,
                        ?total_active,
                        target = "net",
                        "Session established"
                    );

                    this.event_listeners.send(NetworkEvent::SessionEstablished {
                        peer_id,
                        capabilities,
                        messages,
                    });
                }
                SwarmEvent::SessionClosed { node_id: peer_id, remote_addr } => {
                    let total_active = this.num_active_peers.fetch_sub(1, Ordering::Relaxed) - 1;
                    trace!(
                        ?remote_addr,
                        ?peer_id,
                        ?total_active,
                        target = "net",
                        "Session disconnected"
                    );

                    this.event_listeners.send(NetworkEvent::SessionClosed { peer_id });
                }
                SwarmEvent::IncomingPendingSessionClosed { .. } => {}
                SwarmEvent::OutgoingPendingSessionClosed { .. } => {}
                SwarmEvent::OutgoingConnectionError { .. } => {}
            }
        }

        Poll::Pending
    }
}

/// (Non-exhaustive) Events emitted by the network that are of interest for subscribers.
///
/// This includes any event types that may be relevant to tasks, for metrics, keep track of peers
/// etc.
#[derive(Debug, Clone)]
pub enum NetworkEvent {
    /// Closed the peer session.
    SessionClosed { peer_id: PeerId },
    /// Established a new session with the given peer.
    SessionEstablished {
        peer_id: PeerId,
        capabilities: Arc<Capabilities>,
        messages: PeerRequestSender,
    },
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

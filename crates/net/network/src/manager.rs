//! High level network management.
//!
//! The [`NetworkManager`] contains the state of the network as a whole. It controls how connections
//! are handled and keeps track of connections to peers.
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
    eth_requests::IncomingEthRequest,
    import::{BlockImport, BlockImportOutcome, BlockValidation},
    listener::ConnectionListener,
    message::{NewBlockMessage, PeerMessage, PeerRequest, PeerRequestSender},
    network::{NetworkHandle, NetworkHandleMessage},
    peers::{PeersHandle, PeersManager, ReputationChangeKind},
    session::SessionManager,
    state::NetworkState,
    swarm::{Swarm, SwarmEvent},
    transactions::NetworkTransactionEvent,
    FetchClient, NetworkBuilder,
};
use futures::{Future, StreamExt};
use parking_lot::Mutex;
use reth_eth_wire::{
    capability::{Capabilities, CapabilityMessage},
    DisconnectReason, Status,
};
use reth_primitives::{PeerId, H256};
use reth_provider::BlockProvider;
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
use tracing::{error, info, trace, warn};
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
///    transactions(Transactions Task)
///    ethrequest(ETH Request Task)
///    discovery(Discovery Task)
///    subgraph NetworkManager
///      direction LR
///      subgraph Swarm
///          direction TB
///          B1[(Session Manager)]
///          B2[(Connection Lister)]
///          B3[(Network State)]
///      end
///   end
///   handle <--> |request response channel| NetworkManager
///   NetworkManager --> |Network events| events
///   transactions <--> |transactions| NetworkManager
///   ethrequest <--> |ETH request handing| NetworkManager
///   discovery --> |Discovered peers| NetworkManager
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
    /// All listeners for high level network events.
    event_listeners: NetworkEventListeners,
    /// Sender half to send events to the
    /// [`TransactionsManager`](crate::transactions::TransactionsManager) task, if configured.
    to_transactions_manager: Option<mpsc::UnboundedSender<NetworkTransactionEvent>>,
    /// Sender half to send events to the
    /// [`EthRequestHandler`](crate::eth_requests::EthRequestHandler) task, if configured.
    to_eth_request_handler: Option<mpsc::UnboundedSender<IncomingEthRequest>>,
    /// Tracks the number of active session (connected peers).
    ///
    /// This is updated via internal events and shared via `Arc` with the [`NetworkHandle`]
    /// Updated by the `NetworkWorker` and loaded by the `NetworkService`.
    num_active_peers: Arc<AtomicUsize>,
}

// === impl NetworkManager ===
impl<C> NetworkManager<C> {
    /// Sets the dedicated channel for events indented for the
    /// [`TransactionsManager`](crate::transactions::TransactionsManager).
    pub fn set_transactions(&mut self, tx: mpsc::UnboundedSender<NetworkTransactionEvent>) {
        self.to_transactions_manager = Some(tx);
    }

    /// Sets the dedicated channel for events indented for the
    /// [`EthRequestHandler`](crate::eth_requests::EthRequestHandler).
    pub fn set_eth_request_handler(&mut self, tx: mpsc::UnboundedSender<IncomingEthRequest>) {
        self.to_eth_request_handler = Some(tx);
    }

    /// Returns the [`NetworkHandle`] that can be cloned and shared.
    ///
    /// The [`NetworkHandle`] can be used to interact with this [`NetworkManager`]
    pub fn handle(&self) -> &NetworkHandle {
        &self.handle
    }
}

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
            mut discovery_v4_config,
            discovery_addr,
            listener_addr,
            peers_config,
            sessions_config,
            genesis_hash,
            block_import,
            network_mode,
            boot_nodes,
            executor,
            hello_message,
            status,
            fork_filter,
            ..
        } = config;

        let peers_manager = PeersManager::new(peers_config);
        let peers_handle = peers_manager.handle();

        let incoming = ConnectionListener::bind(listener_addr).await?;
        let listener_address = Arc::new(Mutex::new(incoming.local_address()));

        discovery_v4_config = discovery_v4_config.map(|mut disc_config| {
            // merge configured boot nodes
            disc_config.bootstrap_nodes.extend(boot_nodes.clone());
            disc_config.add_eip868_pair("eth", status.forkid);
            disc_config
        });

        let discovery = Discovery::new(discovery_addr, secret_key, discovery_v4_config).await?;
        // need to retrieve the addr here since provided port could be `0`
        let local_peer_id = discovery.local_id();

        let sessions = SessionManager::new(
            secret_key,
            sessions_config,
            executor,
            status,
            hello_message,
            fork_filter,
        );
        let state = NetworkState::new(client, discovery, peers_manager, genesis_hash);

        let swarm = Swarm::new(incoming, sessions, state);

        let (to_manager_tx, from_handle_rx) = mpsc::unbounded_channel();

        let num_active_peers = Arc::new(AtomicUsize::new(0));
        let handle = NetworkHandle::new(
            Arc::clone(&num_active_peers),
            listener_address,
            to_manager_tx,
            local_peer_id,
            peers_handle,
            network_mode,
        );

        Ok(Self {
            swarm,
            handle,
            from_handle_rx: UnboundedReceiverStream::new(from_handle_rx),
            block_import,
            event_listeners: Default::default(),
            to_transactions_manager: None,
            to_eth_request_handler: None,
            num_active_peers,
        })
    }

    /// Create a new [`NetworkManager`] instance and start a [`NetworkBuilder`] to configure all
    /// components of the network
    ///
    /// ```
    /// use reth_provider::test_utils::NoopProvider;
    /// use reth_transaction_pool::TransactionPool;
    /// use std::sync::Arc;
    /// use reth_discv4::bootnodes::mainnet_nodes;
    /// use reth_network::config::rng_secret_key;
    /// use reth_network::{NetworkConfig, NetworkManager};
    /// async fn launch<Pool: TransactionPool>(pool: Pool) {
    ///     // This block provider implementation is used for testing purposes.
    ///     let client = Arc::new(NoopProvider::default());
    ///
    ///     // The key that's used for encrypting sessions and to identify our node.
    ///     let local_key = rng_secret_key();
    ///
    ///     let config =
    ///         NetworkConfig::builder(Arc::clone(&client), local_key).boot_nodes(mainnet_nodes()).build();
    ///
    ///     // create the network instance
    ///     let (handle, network, transactions, request_handler) = NetworkManager::builder(config)
    ///         .await
    ///         .unwrap()
    ///         .transactions(pool)
    ///         .request_handler(client)
    ///         .split_with_handle();
    /// }
    /// ```
    pub async fn builder(
        config: NetworkConfig<C>,
    ) -> Result<NetworkBuilder<C, (), ()>, NetworkError> {
        let network = Self::new(config).await?;
        Ok(network.into_builder())
    }

    /// Create a [`NetworkBuilder`] to configure all components of the network
    pub fn into_builder(self) -> NetworkBuilder<C, (), ()> {
        NetworkBuilder { network: self, transactions: (), request_handler: () }
    }

    /// Returns the [`SocketAddr`] that listens for incoming connections.
    pub fn local_addr(&self) -> SocketAddr {
        self.swarm.listener().local_address()
    }

    /// Returns the configured genesis hash
    pub fn genesis_hash(&self) -> H256 {
        self.swarm.state().genesis_hash()
    }

    /// How many peers we're currently connected to.
    pub fn num_connected_peers(&self) -> usize {
        self.swarm.state().num_active_peers()
    }

    /// Returns the [`PeerId`] used in the network.
    pub fn peer_id(&self) -> &PeerId {
        self.handle.peer_id()
    }

    /// Returns a new [`PeersHandle`] that can be cloned and shared.
    ///
    /// The [`PeersHandle`] can be used to interact with the network's peer set.
    pub fn peers_handle(&self) -> PeersHandle {
        self.swarm.state().peers().handle()
    }

    /// Returns a new [`FetchClient`] that can be cloned and shared.
    ///
    /// The [`FetchClient`] is the entrypoint for sending requests to the network.
    pub fn fetch_client(&self) -> FetchClient {
        self.swarm.state().fetch_client()
    }

    /// Event hook for an unexpected message from the peer.
    fn on_invalid_message(
        &mut self,
        peer_id: PeerId,
        _capabilities: Arc<Capabilities>,
        _message: CapabilityMessage,
    ) {
        trace!(target : "net", ?peer_id,  "received unexpected message");
        self.swarm
            .state_mut()
            .peers_mut()
            .apply_reputation_change(&peer_id, ReputationChangeKind::BadProtocol);
    }

    /// Sends an event to the [`TransactionsManager`](crate::transactions::TransactionsManager) if
    /// configured.
    fn notify_tx_manager(&self, event: NetworkTransactionEvent) {
        if let Some(ref tx) = self.to_transactions_manager {
            let _ = tx.send(event);
        }
    }

    /// Sends an event to the [`EthRequestManager`](crate::eth_requests::EthRequestHandler) if
    /// configured.
    fn delegate_eth_request(&self, event: IncomingEthRequest) {
        if let Some(ref reqs) = self.to_eth_request_handler {
            let _ = reqs.send(event);
        }
    }

    /// Handle an incoming request from the peer
    fn on_eth_request(&mut self, peer_id: PeerId, req: PeerRequest) {
        match req {
            PeerRequest::GetBlockHeaders { request, response } => {
                self.delegate_eth_request(IncomingEthRequest::GetBlockHeaders {
                    peer_id,
                    request,
                    response,
                })
            }
            PeerRequest::GetBlockBodies { request, response } => {
                self.delegate_eth_request(IncomingEthRequest::GetBlockBodies {
                    peer_id,
                    request,
                    response,
                })
            }
            PeerRequest::GetNodeData { request, response } => {
                self.delegate_eth_request(IncomingEthRequest::GetNodeData {
                    peer_id,
                    request,
                    response,
                })
            }
            PeerRequest::GetReceipts { request, response } => {
                self.delegate_eth_request(IncomingEthRequest::GetReceipts {
                    peer_id,
                    request,
                    response,
                })
            }
            PeerRequest::GetPooledTransactions { request, response } => {
                self.notify_tx_manager(NetworkTransactionEvent::GetPooledTransactions {
                    peer_id,
                    request,
                    response,
                });
            }
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
                self.swarm
                    .state_mut()
                    .peers_mut()
                    .apply_reputation_change(&peer, ReputationChangeKind::BadBlock);
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
            self.swarm
                .sessions_mut()
                .disconnect(peer_id, Some(DisconnectReason::SubprotocolSpecific));
        } else {
            only_pow(self);
        }
    }

    /// Handles a received Message from the peer's session.
    fn on_peer_message(&mut self, peer_id: PeerId, msg: PeerMessage) {
        match msg {
            PeerMessage::NewBlockHashes(hashes) => {
                self.within_pow_or_disconnect(peer_id, |this| {
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
            PeerMessage::EthRequest(req) => {
                self.on_eth_request(peer_id, req);
            }
            PeerMessage::ReceivedTransaction(msg) => {
                self.notify_tx_manager(NetworkTransactionEvent::IncomingTransactions {
                    peer_id,
                    msg,
                });
            }
            PeerMessage::SendTransactions(_) => {
                unreachable!("Not emitted by session")
            }
            PeerMessage::Other(other) => {
                error!(target : "net", message_id=%other.id, "Ignoring unsupported message");
            }
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
                    error!(target : "net", "Block propagation is not supported in POS - [EIP-3675](https://eips.ethereum.org/EIPS/eip-3675#devp2p)");
                    return
                }
                let msg = NewBlockMessage { hash, block: Arc::new(block) };
                self.swarm.state_mut().announce_new_block(msg);
            }
            NetworkHandleMessage::EthRequest { peer_id, request } => {
                self.swarm.sessions_mut().send_message(&peer_id, PeerMessage::EthRequest(request))
            }
            NetworkHandleMessage::SendTransaction { peer_id, msg } => {
                self.swarm.sessions_mut().send_message(&peer_id, PeerMessage::SendTransactions(msg))
            }
            NetworkHandleMessage::SendPooledTransactionHashes { peer_id, msg } => self
                .swarm
                .sessions_mut()
                .send_message(&peer_id, PeerMessage::PooledTransactions(msg)),
            NetworkHandleMessage::AddPeerAddress(peer, kind, addr) => {
                self.swarm.state_mut().add_peer_kind(peer, kind, addr);
            }
            NetworkHandleMessage::RemovePeer(peer_id, kind) => {
                self.swarm.state_mut().remove_peer(peer_id, kind);
            }
            NetworkHandleMessage::DisconnectPeer(peer_id, reason) => {
                self.swarm.sessions_mut().disconnect(peer_id, reason);
            }
            NetworkHandleMessage::ReputationChange(peer_id, kind) => {
                self.swarm.state_mut().peers_mut().apply_reputation_change(&peer_id, kind);
            }
            NetworkHandleMessage::FetchClient(tx) => {
                let _ = tx.send(self.fetch_client());
            }
            NetworkHandleMessage::StatusUpdate { height, hash, total_difficulty } => {
                if let Some(transition) =
                    self.swarm.sessions_mut().on_status_update(height, hash, total_difficulty)
                {
                    self.swarm.state_mut().update_fork_id(transition.current);
                }
            }
            NetworkHandleMessage::GetPeerInfo(tx) => {
                let _ = tx.send(self.swarm.sessions_mut().get_peer_info());
            }
            NetworkHandleMessage::GetPeerInfoById(peer_id, tx) => {
                let _ = tx.send(self.swarm.sessions_mut().get_peer_info_by_id(peer_id));
            }
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
                SwarmEvent::ValidMessage { peer_id, message } => {
                    this.on_peer_message(peer_id, message)
                }
                SwarmEvent::InvalidCapabilityMessage { peer_id, capabilities, message } => {
                    this.on_invalid_message(peer_id, capabilities, message)
                }
                SwarmEvent::TcpListenerClosed { remote_addr } => {
                    trace!(target : "net", ?remote_addr, "TCP listener closed.");
                }
                SwarmEvent::TcpListenerError(err) => {
                    trace!(target : "net", ?err, "TCP connection error.");
                }
                SwarmEvent::IncomingTcpConnection { remote_addr, session_id } => {
                    trace!(target : "net", ?session_id, ?remote_addr, "Incoming connection");
                }
                SwarmEvent::OutgoingTcpConnection { remote_addr, peer_id } => {
                    trace!(target : "net", ?remote_addr, ?peer_id, "Starting outbound connection.");
                }
                SwarmEvent::SessionEstablished {
                    peer_id,
                    remote_addr,
                    capabilities,
                    messages,
                    status,
                    direction,
                } => {
                    let total_active = this.num_active_peers.fetch_add(1, Ordering::Relaxed) + 1;
                    info!(
                        target : "net",
                        ?remote_addr,
                        ?peer_id,
                        ?total_active,
                        "Session established"
                    );

                    if direction.is_incoming() {
                        this.swarm
                            .state_mut()
                            .peers_mut()
                            .on_active_inbound_session(peer_id, remote_addr);
                    }

                    this.event_listeners.send(NetworkEvent::SessionEstablished {
                        peer_id,
                        capabilities,
                        status,
                        messages,
                    });
                }
                SwarmEvent::PeerAdded(peer_id) => {
                    info!(target: "net", ?peer_id, "Peer added");
                    this.event_listeners.send(NetworkEvent::PeerAdded(peer_id));
                }
                SwarmEvent::PeerRemoved(peer_id) => {
                    info!(target: "net", ?peer_id, "Peer dropped");
                    this.event_listeners.send(NetworkEvent::PeerRemoved(peer_id));
                }
                SwarmEvent::SessionClosed { peer_id, remote_addr, error } => {
                    let total_active = this.num_active_peers.fetch_sub(1, Ordering::Relaxed) - 1;
                    trace!(
                        target : "net",
                        ?remote_addr,
                        ?peer_id,
                        ?total_active,
                        ?error,
                        "Session disconnected"
                    );

                    let mut reason = None;
                    if let Some(ref err) = error {
                        // If the connection was closed due to an error, we report the peer
                        this.swarm.state_mut().peers_mut().on_active_session_dropped(
                            &remote_addr,
                            &peer_id,
                            err,
                        );
                        reason = err.as_disconnected();
                    } else {
                        // Gracefully disconnected
                        this.swarm
                            .state_mut()
                            .peers_mut()
                            .on_active_session_gracefully_closed(peer_id);
                    }

                    this.event_listeners.send(NetworkEvent::SessionClosed { peer_id, reason });
                }
                SwarmEvent::IncomingPendingSessionClosed { remote_addr, error } => {
                    warn!(
                        target : "net",
                        ?remote_addr,
                        ?error,
                        "Incoming pending session failed"
                    );

                    if let Some(ref err) = error {
                        this.swarm
                            .state_mut()
                            .peers_mut()
                            .on_incoming_pending_session_dropped(remote_addr, err);
                    } else {
                        this.swarm
                            .state_mut()
                            .peers_mut()
                            .on_incoming_pending_session_gracefully_closed();
                    }
                }
                SwarmEvent::OutgoingPendingSessionClosed { remote_addr, peer_id, error } => {
                    warn!(
                        target : "net",
                        ?remote_addr,
                        ?peer_id,
                        ?error,
                        "Outgoing pending session failed"
                    );

                    if let Some(ref err) = error {
                        this.swarm.state_mut().peers_mut().on_pending_session_dropped(
                            &remote_addr,
                            &peer_id,
                            err,
                        );
                    } else {
                        this.swarm
                            .state_mut()
                            .peers_mut()
                            .on_pending_session_gracefully_closed(&peer_id);
                    }
                }
                SwarmEvent::OutgoingConnectionError { remote_addr, peer_id, error } => {
                    warn!(
                        target : "net",
                        ?remote_addr,
                        ?peer_id,
                        ?error,
                        "Outgoing connection error"
                    );
                    this.swarm
                        .state_mut()
                        .peers_mut()
                        .apply_reputation_change(&peer_id, ReputationChangeKind::FailedToConnect);
                }
                SwarmEvent::BadMessage { peer_id } => {
                    this.swarm
                        .state_mut()
                        .peers_mut()
                        .apply_reputation_change(&peer_id, ReputationChangeKind::FailedToConnect);
                }
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
    SessionClosed {
        /// The identifier of the peer to which a session was closed.
        peer_id: PeerId,
        /// Why the disconnect was triggered
        reason: Option<DisconnectReason>,
    },
    /// Established a new session with the given peer.
    SessionEstablished {
        /// The identifier of the peer to which a session was established.
        peer_id: PeerId,
        /// Capabilities the peer announced
        capabilities: Arc<Capabilities>,
        /// A request channel to the session task.
        messages: PeerRequestSender,
        /// The status of the peer to which a session was established.
        status: Status,
    },
    /// Event emitted when a new peer is added
    PeerAdded(PeerId),
    /// Event emitted when a new peer is removed
    PeerRemoved(PeerId),
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
                trace!(target : "net", "event listener channel closed",);
            }
            open
        });
    }
}

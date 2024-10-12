//! High level network management.
//!
//! The [`NetworkManager`] contains the state of the network as a whole. It controls how connections
//! are handled and keeps track of connections to peers.
//!
//! ## Capabilities
//!
//! The network manages peers depending on their announced capabilities via their `RLPx` sessions. Most importantly the [Ethereum Wire Protocol](https://github.com/ethereum/devp2p/blob/master/caps/eth.md)(`eth`).
//!
//! ## Overview
//!
//! The [`NetworkManager`] is responsible for advancing the state of the `network`. The `network` is
//! made up of peer-to-peer connections between nodes that are available on the same network.
//! Responsible for peer discovery is ethereum's discovery protocol (discv4, discv5). If the address
//! (IP+port) of our node is published via discovery, remote peers can initiate inbound connections
//! to the local node. Once a (tcp) connection is established, both peers start to authenticate a [RLPx session](https://github.com/ethereum/devp2p/blob/master/rlpx.md) via a handshake. If the handshake was successful, both peers announce their capabilities and are now ready to exchange sub-protocol messages via the `RLPx` session.

use std::{
    net::SocketAddr,
    path::Path,
    pin::Pin,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc,
    },
    task::{Context, Poll},
    time::{Duration, Instant},
};

use futures::{Future, StreamExt};
use parking_lot::Mutex;
use reth_eth_wire::{capability::CapabilityMessage, Capabilities, DisconnectReason};
use reth_fs_util::{self as fs, FsPathError};
use reth_metrics::common::mpsc::UnboundedMeteredSender;
use reth_network_api::{
    test_utils::PeersHandle, EthProtocolInfo, NetworkEvent, NetworkStatus, PeerInfo, PeerRequest,
};
use reth_network_peers::{NodeRecord, PeerId};
use reth_network_types::ReputationChangeKind;
use reth_storage_api::BlockNumReader;
use reth_tasks::shutdown::GracefulShutdown;
use reth_tokio_util::EventSender;
use secp256k1::SecretKey;
use tokio::sync::mpsc::{self, error::TrySendError};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::{debug, error, trace, warn};

use crate::{
    budget::{DEFAULT_BUDGET_TRY_DRAIN_NETWORK_HANDLE_CHANNEL, DEFAULT_BUDGET_TRY_DRAIN_SWARM},
    config::NetworkConfig,
    discovery::Discovery,
    error::{NetworkError, ServiceKind},
    eth_requests::IncomingEthRequest,
    import::{BlockImport, BlockImportOutcome, BlockValidation},
    listener::ConnectionListener,
    message::{NewBlockMessage, PeerMessage},
    metrics::{DisconnectMetrics, NetworkMetrics, NETWORK_POOL_TRANSACTIONS_SCOPE},
    network::{NetworkHandle, NetworkHandleMessage},
    peers::PeersManager,
    poll_nested_stream_with_budget,
    protocol::IntoRlpxSubProtocol,
    session::SessionManager,
    state::NetworkState,
    swarm::{Swarm, SwarmEvent},
    transactions::NetworkTransactionEvent,
    FetchClient, NetworkBuilder,
};

#[cfg_attr(doc, aquamarine::aquamarine)]
/// Manages the _entire_ state of the network.
///
/// This is an endless [`Future`] that consistently drives the state of the entire network forward.
///
/// The [`NetworkManager`] is the container type for all parts involved with advancing the network.
///
/// include_mmd!("docs/mermaid/network-manager.mmd")
#[derive(Debug)]
#[must_use = "The NetworkManager does nothing unless polled"]
pub struct NetworkManager {
    /// The type that manages the actual network part, which includes connections.
    swarm: Swarm,
    /// Underlying network handle that can be shared.
    handle: NetworkHandle,
    /// Receiver half of the command channel set up between this type and the [`NetworkHandle`]
    from_handle_rx: UnboundedReceiverStream<NetworkHandleMessage>,
    /// Handles block imports according to the `eth` protocol.
    block_import: Box<dyn BlockImport>,
    /// Sender for high level network events.
    event_sender: EventSender<NetworkEvent>,
    /// Sender half to send events to the
    /// [`TransactionsManager`](crate::transactions::TransactionsManager) task, if configured.
    to_transactions_manager: Option<UnboundedMeteredSender<NetworkTransactionEvent>>,
    /// Sender half to send events to the
    /// [`EthRequestHandler`](crate::eth_requests::EthRequestHandler) task, if configured.
    ///
    /// The channel that originally receives and bundles all requests from all sessions is already
    /// bounded. However, since handling an eth request is more I/O intensive than delegating
    /// them from the bounded channel to the eth-request channel, it is possible that this
    /// builds up if the node is flooded with requests.
    ///
    /// Even though nonmalicious requests are relatively cheap, it's possible to craft
    /// body requests with bogus data up until the allowed max message size limit.
    /// Thus, we use a bounded channel here to avoid unbounded build up if the node is flooded with
    /// requests. This channel size is set at
    /// [`ETH_REQUEST_CHANNEL_CAPACITY`](crate::builder::ETH_REQUEST_CHANNEL_CAPACITY)
    to_eth_request_handler: Option<mpsc::Sender<IncomingEthRequest>>,
    /// Tracks the number of active session (connected peers).
    ///
    /// This is updated via internal events and shared via `Arc` with the [`NetworkHandle`]
    /// Updated by the `NetworkWorker` and loaded by the `NetworkService`.
    num_active_peers: Arc<AtomicUsize>,
    /// Metrics for the Network
    metrics: NetworkMetrics,
    /// Disconnect metrics for the Network
    disconnect_metrics: DisconnectMetrics,
}

// === impl NetworkManager ===
impl NetworkManager {
    /// Sets the dedicated channel for events indented for the
    /// [`TransactionsManager`](crate::transactions::TransactionsManager).
    pub fn set_transactions(&mut self, tx: mpsc::UnboundedSender<NetworkTransactionEvent>) {
        self.to_transactions_manager =
            Some(UnboundedMeteredSender::new(tx, NETWORK_POOL_TRANSACTIONS_SCOPE));
    }

    /// Sets the dedicated channel for events indented for the
    /// [`EthRequestHandler`](crate::eth_requests::EthRequestHandler).
    pub fn set_eth_request_handler(&mut self, tx: mpsc::Sender<IncomingEthRequest>) {
        self.to_eth_request_handler = Some(tx);
    }

    /// Adds an additional protocol handler to the `RLPx` sub-protocol list.
    pub fn add_rlpx_sub_protocol(&mut self, protocol: impl IntoRlpxSubProtocol) {
        self.swarm.add_rlpx_sub_protocol(protocol)
    }

    /// Returns the [`NetworkHandle`] that can be cloned and shared.
    ///
    /// The [`NetworkHandle`] can be used to interact with this [`NetworkManager`]
    pub const fn handle(&self) -> &NetworkHandle {
        &self.handle
    }

    /// Returns the secret key used for authenticating sessions.
    pub const fn secret_key(&self) -> SecretKey {
        self.swarm.sessions().secret_key()
    }

    #[inline]
    fn update_poll_metrics(&self, start: Instant, poll_durations: NetworkManagerPollDurations) {
        let metrics = &self.metrics;

        let NetworkManagerPollDurations { acc_network_handle, acc_swarm } = poll_durations;

        // update metrics for whole poll function
        metrics.duration_poll_network_manager.set(start.elapsed().as_secs_f64());
        // update poll metrics for nested items
        metrics.acc_duration_poll_network_handle.set(acc_network_handle.as_secs_f64());
        metrics.acc_duration_poll_swarm.set(acc_swarm.as_secs_f64());
    }

    /// Creates the manager of a new network.
    ///
    /// The [`NetworkManager`] is an endless future that needs to be polled in order to advance the
    /// state of the entire network.
    pub async fn new<C: BlockNumReader + 'static>(
        config: NetworkConfig<C>,
    ) -> Result<Self, NetworkError> {
        let NetworkConfig {
            client,
            secret_key,
            discovery_v4_addr,
            mut discovery_v4_config,
            mut discovery_v5_config,
            listener_addr,
            peers_config,
            sessions_config,
            chain_id,
            block_import,
            network_mode,
            boot_nodes,
            executor,
            hello_message,
            status,
            fork_filter,
            dns_discovery_config,
            extra_protocols,
            tx_gossip_disabled,
            transactions_manager_config: _,
            nat,
        } = config;

        let peers_manager = PeersManager::new(peers_config);
        let peers_handle = peers_manager.handle();

        let incoming = ConnectionListener::bind(listener_addr).await.map_err(|err| {
            NetworkError::from_io_error(err, ServiceKind::Listener(listener_addr))
        })?;

        // retrieve the tcp address of the socket
        let listener_addr = incoming.local_address();

        // resolve boot nodes
        let resolved_boot_nodes =
            futures::future::try_join_all(boot_nodes.iter().map(|record| record.resolve())).await?;

        if let Some(disc_config) = discovery_v4_config.as_mut() {
            // merge configured boot nodes
            disc_config.bootstrap_nodes.extend(resolved_boot_nodes.clone());
            disc_config.add_eip868_pair("eth", status.forkid);
        }

        if let Some(discv5) = discovery_v5_config.as_mut() {
            // merge configured boot nodes
            discv5.extend_unsigned_boot_nodes(resolved_boot_nodes)
        }

        let discovery = Discovery::new(
            listener_addr,
            discovery_v4_addr,
            secret_key,
            discovery_v4_config,
            discovery_v5_config,
            dns_discovery_config,
        )
        .await?;
        // need to retrieve the addr here since provided port could be `0`
        let local_peer_id = discovery.local_id();
        let discv4 = discovery.discv4();
        let discv5 = discovery.discv5();

        let num_active_peers = Arc::new(AtomicUsize::new(0));

        let sessions = SessionManager::new(
            secret_key,
            sessions_config,
            executor,
            status,
            hello_message,
            fork_filter,
            extra_protocols,
        );

        let state = NetworkState::new(
            crate::state::BlockNumReader::new(client),
            discovery,
            peers_manager,
            Arc::clone(&num_active_peers),
        );

        let swarm = Swarm::new(incoming, sessions, state);

        let (to_manager_tx, from_handle_rx) = mpsc::unbounded_channel();

        let event_sender: EventSender<NetworkEvent> = Default::default();

        let handle = NetworkHandle::new(
            Arc::clone(&num_active_peers),
            Arc::new(Mutex::new(listener_addr)),
            to_manager_tx,
            secret_key,
            local_peer_id,
            peers_handle,
            network_mode,
            Arc::new(AtomicU64::new(chain_id)),
            tx_gossip_disabled,
            discv4,
            discv5,
            event_sender.clone(),
            nat,
        );

        Ok(Self {
            swarm,
            handle,
            from_handle_rx: UnboundedReceiverStream::new(from_handle_rx),
            block_import,
            event_sender,
            to_transactions_manager: None,
            to_eth_request_handler: None,
            num_active_peers,
            metrics: Default::default(),
            disconnect_metrics: Default::default(),
        })
    }

    /// Create a new [`NetworkManager`] instance and start a [`NetworkBuilder`] to configure all
    /// components of the network
    ///
    /// ```
    /// use reth_network::{config::rng_secret_key, NetworkConfig, NetworkManager};
    /// use reth_network_peers::mainnet_nodes;
    /// use reth_provider::test_utils::NoopProvider;
    /// use reth_transaction_pool::TransactionPool;
    /// async fn launch<Pool: TransactionPool>(pool: Pool) {
    ///     // This block provider implementation is used for testing purposes.
    ///     let client = NoopProvider::default();
    ///
    ///     // The key that's used for encrypting sessions and to identify our node.
    ///     let local_key = rng_secret_key();
    ///
    ///     let config =
    ///         NetworkConfig::builder(local_key).boot_nodes(mainnet_nodes()).build(client.clone());
    ///     let transactions_manager_config = config.transactions_manager_config.clone();
    ///
    ///     // create the network instance
    ///     let (handle, network, transactions, request_handler) = NetworkManager::builder(config)
    ///         .await
    ///         .unwrap()
    ///         .transactions(pool, transactions_manager_config)
    ///         .request_handler(client)
    ///         .split_with_handle();
    /// }
    /// ```
    pub async fn builder<C: BlockNumReader + 'static>(
        config: NetworkConfig<C>,
    ) -> Result<NetworkBuilder<(), ()>, NetworkError> {
        let network = Self::new(config).await?;
        Ok(network.into_builder())
    }

    /// Create a [`NetworkBuilder`] to configure all components of the network
    pub const fn into_builder(self) -> NetworkBuilder<(), ()> {
        NetworkBuilder { network: self, transactions: (), request_handler: () }
    }

    /// Returns the [`SocketAddr`] that listens for incoming tcp connections.
    pub const fn local_addr(&self) -> SocketAddr {
        self.swarm.listener().local_address()
    }

    /// How many peers we're currently connected to.
    pub fn num_connected_peers(&self) -> usize {
        self.swarm.state().num_active_peers()
    }

    /// Returns the [`PeerId`] used in the network.
    pub fn peer_id(&self) -> &PeerId {
        self.handle.peer_id()
    }

    /// Returns an iterator over all peers in the peer set.
    pub fn all_peers(&self) -> impl Iterator<Item = NodeRecord> + '_ {
        self.swarm.state().peers().iter_peers()
    }

    /// Returns the number of peers in the peer set.
    pub fn num_known_peers(&self) -> usize {
        self.swarm.state().peers().num_known_peers()
    }

    /// Returns a new [`PeersHandle`] that can be cloned and shared.
    ///
    /// The [`PeersHandle`] can be used to interact with the network's peer set.
    pub fn peers_handle(&self) -> PeersHandle {
        self.swarm.state().peers().handle()
    }

    /// Collect the peers from the [`NetworkManager`] and write them to the given
    /// `persistent_peers_file`.
    pub fn write_peers_to_file(&self, persistent_peers_file: &Path) -> Result<(), FsPathError> {
        let known_peers = self.all_peers().collect::<Vec<_>>();
        persistent_peers_file.parent().map(fs::create_dir_all).transpose()?;
        reth_fs_util::write_json_file(persistent_peers_file, &known_peers)?;
        Ok(())
    }

    /// Returns a new [`FetchClient`] that can be cloned and shared.
    ///
    /// The [`FetchClient`] is the entrypoint for sending requests to the network.
    pub fn fetch_client(&self) -> FetchClient {
        self.swarm.state().fetch_client()
    }

    /// Returns the current [`NetworkStatus`] for the local node.
    pub fn status(&self) -> NetworkStatus {
        let sessions = self.swarm.sessions();
        let status = sessions.status();
        let hello_message = sessions.hello_message();

        NetworkStatus {
            client_version: hello_message.client_version,
            protocol_version: hello_message.protocol_version as u64,
            eth_protocol_info: EthProtocolInfo {
                difficulty: status.total_difficulty,
                head: status.blockhash,
                network: status.chain.id(),
                genesis: status.genesis,
                config: Default::default(),
            },
        }
    }

    /// Event hook for an unexpected message from the peer.
    fn on_invalid_message(
        &mut self,
        peer_id: PeerId,
        _capabilities: Arc<Capabilities>,
        _message: CapabilityMessage,
    ) {
        trace!(target: "net", ?peer_id, "received unexpected message");
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
            let _ = reqs.try_send(event).map_err(|e| {
                if let TrySendError::Full(_) = e {
                    debug!(target:"net", "EthRequestHandler channel is full!");
                    self.metrics.total_dropped_eth_requests_at_full_capacity.increment(1);
                }
            });
        }
    }

    /// Handle an incoming request from the peer
    fn on_eth_request(&self, peer_id: PeerId, req: PeerRequest) {
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
                debug!(target: "net", message_id=%other.id, "Ignoring unsupported message");
            }
        }
    }

    /// Handler for received messages from a handle
    fn on_handle_message(&mut self, msg: NetworkHandleMessage) {
        match msg {
            NetworkHandleMessage::DiscoveryListener(tx) => {
                self.swarm.state_mut().discovery_mut().add_listener(tx);
            }
            NetworkHandleMessage::AnnounceBlock(block, hash) => {
                if self.handle.mode().is_stake() {
                    // See [EIP-3675](https://eips.ethereum.org/EIPS/eip-3675#devp2p)
                    warn!(target: "net", "Peer performed block propagation, but it is not supported in proof of stake (EIP-3675)");
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
            NetworkHandleMessage::AddTrustedPeerId(peer_id) => {
                self.swarm.state_mut().add_trusted_peer_id(peer_id);
            }
            NetworkHandleMessage::AddPeerAddress(peer, kind, addr) => {
                // only add peer if we are not shutting down
                if !self.swarm.is_shutting_down() {
                    self.swarm.state_mut().add_peer_kind(peer, kind, addr);
                }
            }
            NetworkHandleMessage::RemovePeer(peer_id, kind) => {
                self.swarm.state_mut().remove_peer_kind(peer_id, kind);
            }
            NetworkHandleMessage::DisconnectPeer(peer_id, reason) => {
                self.swarm.sessions_mut().disconnect(peer_id, reason);
            }
            NetworkHandleMessage::ConnectPeer(peer_id, kind, addr) => {
                self.swarm.state_mut().add_and_connect(peer_id, kind, addr);
            }
            NetworkHandleMessage::SetNetworkState(net_state) => {
                // Sets network connection state between Active and Hibernate.
                // If hibernate stops the node to fill new outbound
                // connections, this is beneficial for sync stages that do not require a network
                // connection.
                self.swarm.on_network_state_change(net_state);
            }

            NetworkHandleMessage::Shutdown(tx) => {
                self.perform_network_shutdown();
                let _ = tx.send(());
            }
            NetworkHandleMessage::ReputationChange(peer_id, kind) => {
                self.swarm.state_mut().peers_mut().apply_reputation_change(&peer_id, kind);
            }
            NetworkHandleMessage::GetReputationById(peer_id, tx) => {
                let _ = tx.send(self.swarm.state_mut().peers().get_reputation(&peer_id));
            }
            NetworkHandleMessage::FetchClient(tx) => {
                let _ = tx.send(self.fetch_client());
            }
            NetworkHandleMessage::GetStatus(tx) => {
                let _ = tx.send(self.status());
            }
            NetworkHandleMessage::StatusUpdate { head } => {
                if let Some(transition) = self.swarm.sessions_mut().on_status_update(head) {
                    self.swarm.state_mut().update_fork_id(transition.current);
                }
            }
            NetworkHandleMessage::GetPeerInfos(tx) => {
                let _ = tx.send(self.get_peer_infos());
            }
            NetworkHandleMessage::GetPeerInfoById(peer_id, tx) => {
                let _ = tx.send(self.get_peer_info_by_id(peer_id));
            }
            NetworkHandleMessage::GetPeerInfosByIds(peer_ids, tx) => {
                let _ = tx.send(self.get_peer_infos_by_ids(peer_ids));
            }
            NetworkHandleMessage::GetPeerInfosByPeerKind(kind, tx) => {
                let peer_ids = self.swarm.state().peers().peers_by_kind(kind);
                let _ = tx.send(self.get_peer_infos_by_ids(peer_ids));
            }
            NetworkHandleMessage::AddRlpxSubProtocol(proto) => self.add_rlpx_sub_protocol(proto),
            NetworkHandleMessage::GetTransactionsHandle(tx) => {
                if let Some(ref tx_inner) = self.to_transactions_manager {
                    let _ = tx_inner.send(NetworkTransactionEvent::GetTransactionsHandle(tx));
                } else {
                    let _ = tx.send(None);
                }
            }
        }
    }

    fn on_swarm_event(&mut self, event: SwarmEvent) {
        // handle event
        match event {
            SwarmEvent::ValidMessage { peer_id, message } => self.on_peer_message(peer_id, message),
            SwarmEvent::InvalidCapabilityMessage { peer_id, capabilities, message } => {
                self.on_invalid_message(peer_id, capabilities, message);
                self.metrics.invalid_messages_received.increment(1);
            }
            SwarmEvent::TcpListenerClosed { remote_addr } => {
                trace!(target: "net", ?remote_addr, "TCP listener closed.");
            }
            SwarmEvent::TcpListenerError(err) => {
                trace!(target: "net", %err, "TCP connection error.");
            }
            SwarmEvent::IncomingTcpConnection { remote_addr, session_id } => {
                trace!(target: "net", ?session_id, ?remote_addr, "Incoming connection");
                self.metrics.total_incoming_connections.increment(1);
                self.metrics
                    .incoming_connections
                    .set(self.swarm.state().peers().num_inbound_connections() as f64);
            }
            SwarmEvent::OutgoingTcpConnection { remote_addr, peer_id } => {
                trace!(target: "net", ?remote_addr, ?peer_id, "Starting outbound connection.");
                self.metrics.total_outgoing_connections.increment(1);
                self.update_pending_connection_metrics()
            }
            SwarmEvent::SessionEstablished {
                peer_id,
                remote_addr,
                client_version,
                capabilities,
                version,
                messages,
                status,
                direction,
            } => {
                let total_active = self.num_active_peers.fetch_add(1, Ordering::Relaxed) + 1;
                self.metrics.connected_peers.set(total_active as f64);
                debug!(
                    target: "net",
                    ?remote_addr,
                    %client_version,
                    ?peer_id,
                    ?total_active,
                    kind=%direction,
                    peer_enode=%NodeRecord::new(remote_addr, peer_id),
                    "Session established"
                );

                if direction.is_incoming() {
                    self.swarm
                        .state_mut()
                        .peers_mut()
                        .on_incoming_session_established(peer_id, remote_addr);
                }

                if direction.is_outgoing() {
                    self.swarm.state_mut().peers_mut().on_active_outgoing_established(peer_id);
                }

                self.update_active_connection_metrics();

                self.event_sender.notify(NetworkEvent::SessionEstablished {
                    peer_id,
                    remote_addr,
                    client_version,
                    capabilities,
                    version,
                    status,
                    messages,
                });
            }
            SwarmEvent::PeerAdded(peer_id) => {
                trace!(target: "net", ?peer_id, "Peer added");
                self.event_sender.notify(NetworkEvent::PeerAdded(peer_id));
                self.metrics.tracked_peers.set(self.swarm.state().peers().num_known_peers() as f64);
            }
            SwarmEvent::PeerRemoved(peer_id) => {
                trace!(target: "net", ?peer_id, "Peer dropped");
                self.event_sender.notify(NetworkEvent::PeerRemoved(peer_id));
                self.metrics.tracked_peers.set(self.swarm.state().peers().num_known_peers() as f64);
            }
            SwarmEvent::SessionClosed { peer_id, remote_addr, error } => {
                let total_active = self.num_active_peers.fetch_sub(1, Ordering::Relaxed) - 1;
                self.metrics.connected_peers.set(total_active as f64);
                trace!(
                    target: "net",
                    ?remote_addr,
                    ?peer_id,
                    ?total_active,
                    ?error,
                    "Session disconnected"
                );

                let mut reason = None;
                if let Some(ref err) = error {
                    // If the connection was closed due to an error, we report
                    // the peer
                    self.swarm.state_mut().peers_mut().on_active_session_dropped(
                        &remote_addr,
                        &peer_id,
                        err,
                    );
                    reason = err.as_disconnected();
                } else {
                    // Gracefully disconnected
                    self.swarm.state_mut().peers_mut().on_active_session_gracefully_closed(peer_id);
                }
                self.metrics.closed_sessions.increment(1);
                self.update_active_connection_metrics();

                if let Some(reason) = reason {
                    self.disconnect_metrics.increment(reason);
                }
                self.metrics.backed_off_peers.set(
                        self.swarm
                            .state()
                            .peers()
                            .num_backed_off_peers()
                            .saturating_sub(1)
                            as f64,
                    );
                self.event_sender.notify(NetworkEvent::SessionClosed { peer_id, reason });
            }
            SwarmEvent::IncomingPendingSessionClosed { remote_addr, error } => {
                trace!(
                    target: "net",
                    ?remote_addr,
                    ?error,
                    "Incoming pending session failed"
                );

                if let Some(ref err) = error {
                    self.swarm
                        .state_mut()
                        .peers_mut()
                        .on_incoming_pending_session_dropped(remote_addr, err);
                    self.metrics.pending_session_failures.increment(1);
                    if let Some(reason) = err.as_disconnected() {
                        self.disconnect_metrics.increment(reason);
                    }
                } else {
                    self.swarm
                        .state_mut()
                        .peers_mut()
                        .on_incoming_pending_session_gracefully_closed();
                }
                self.metrics.closed_sessions.increment(1);
                self.metrics
                    .incoming_connections
                    .set(self.swarm.state().peers().num_inbound_connections() as f64);
                self.metrics.backed_off_peers.set(
                        self.swarm
                            .state()
                            .peers()
                            .num_backed_off_peers()
                            .saturating_sub(1)
                            as f64,
                    );
            }
            SwarmEvent::OutgoingPendingSessionClosed { remote_addr, peer_id, error } => {
                trace!(
                    target: "net",
                    ?remote_addr,
                    ?peer_id,
                    ?error,
                    "Outgoing pending session failed"
                );

                if let Some(ref err) = error {
                    self.swarm.state_mut().peers_mut().on_outgoing_pending_session_dropped(
                        &remote_addr,
                        &peer_id,
                        err,
                    );
                    self.metrics.pending_session_failures.increment(1);
                    if let Some(reason) = err.as_disconnected() {
                        self.disconnect_metrics.increment(reason);
                    }
                } else {
                    self.swarm
                        .state_mut()
                        .peers_mut()
                        .on_outgoing_pending_session_gracefully_closed(&peer_id);
                }
                self.metrics.closed_sessions.increment(1);
                self.update_pending_connection_metrics();

                self.metrics.backed_off_peers.set(
                        self.swarm
                            .state()
                            .peers()
                            .num_backed_off_peers()
                            .saturating_sub(1)
                            as f64,
                    );
            }
            SwarmEvent::OutgoingConnectionError { remote_addr, peer_id, error } => {
                trace!(
                    target: "net",
                    ?remote_addr,
                    ?peer_id,
                    %error,
                    "Outgoing connection error"
                );

                self.swarm.state_mut().peers_mut().on_outgoing_connection_failure(
                    &remote_addr,
                    &peer_id,
                    &error,
                );

                self.metrics.backed_off_peers.set(
                        self.swarm
                            .state()
                            .peers()
                            .num_backed_off_peers()
                            .saturating_sub(1)
                            as f64,
                    );
                self.update_pending_connection_metrics();
            }
            SwarmEvent::BadMessage { peer_id } => {
                self.swarm
                    .state_mut()
                    .peers_mut()
                    .apply_reputation_change(&peer_id, ReputationChangeKind::BadMessage);
                self.metrics.invalid_messages_received.increment(1);
            }
            SwarmEvent::ProtocolBreach { peer_id } => {
                self.swarm
                    .state_mut()
                    .peers_mut()
                    .apply_reputation_change(&peer_id, ReputationChangeKind::BadProtocol);
            }
        }
    }

    /// Returns [`PeerInfo`] for all connected peers
    fn get_peer_infos(&self) -> Vec<PeerInfo> {
        self.swarm
            .sessions()
            .active_sessions()
            .iter()
            .filter_map(|(&peer_id, session)| {
                self.swarm
                    .state()
                    .peers()
                    .peer_by_id(peer_id)
                    .map(|(record, kind)| session.peer_info(&record, kind))
            })
            .collect()
    }

    /// Returns [`PeerInfo`] for a given peer.
    ///
    /// Returns `None` if there's no active session to the peer.
    fn get_peer_info_by_id(&self, peer_id: PeerId) -> Option<PeerInfo> {
        self.swarm.sessions().active_sessions().get(&peer_id).and_then(|session| {
            self.swarm
                .state()
                .peers()
                .peer_by_id(peer_id)
                .map(|(record, kind)| session.peer_info(&record, kind))
        })
    }

    /// Returns [`PeerInfo`] for a given peers.
    ///
    /// Ignore the non-active peer.
    fn get_peer_infos_by_ids(&self, peer_ids: impl IntoIterator<Item = PeerId>) -> Vec<PeerInfo> {
        peer_ids.into_iter().filter_map(|peer_id| self.get_peer_info_by_id(peer_id)).collect()
    }

    /// Updates the metrics for active,established connections
    #[inline]
    fn update_active_connection_metrics(&self) {
        self.metrics
            .incoming_connections
            .set(self.swarm.state().peers().num_inbound_connections() as f64);
        self.metrics
            .outgoing_connections
            .set(self.swarm.state().peers().num_outbound_connections() as f64);
    }

    /// Updates the metrics for pending connections
    #[inline]
    fn update_pending_connection_metrics(&self) {
        self.metrics
            .pending_outgoing_connections
            .set(self.swarm.state().peers().num_pending_outbound_connections() as f64);
        self.metrics
            .total_pending_connections
            .set(self.swarm.sessions().num_pending_connections() as f64);
    }

    /// Drives the [`NetworkManager`] future until a [`GracefulShutdown`] signal is received.
    ///
    /// This invokes the given function `shutdown_hook` while holding the graceful shutdown guard.
    pub async fn run_until_graceful_shutdown<F, R>(
        mut self,
        shutdown: GracefulShutdown,
        shutdown_hook: F,
    ) -> R
    where
        F: FnOnce(Self) -> R,
    {
        let mut graceful_guard = None;
        tokio::select! {
            _ = &mut self => {},
            guard = shutdown => {
                graceful_guard = Some(guard);
            },
        }

        self.perform_network_shutdown();
        let res = shutdown_hook(self);
        drop(graceful_guard);
        res
    }

    /// Performs a graceful network shutdown by stopping new connections from being accepted while
    /// draining current and pending connections.
    fn perform_network_shutdown(&mut self) {
        // Set connection status to `Shutdown`. Stops node from accepting
        // new incoming connections as well as sending connection requests to newly
        // discovered nodes.
        self.swarm.on_shutdown_requested();
        // Disconnect all active connections
        self.swarm.sessions_mut().disconnect_all(Some(DisconnectReason::ClientQuitting));
        // drop pending connections
        self.swarm.sessions_mut().disconnect_all_pending();
    }
}

impl Future for NetworkManager {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let start = Instant::now();
        let mut poll_durations = NetworkManagerPollDurations::default();

        let this = self.get_mut();

        // poll new block imports (expected to be a noop for POS)
        while let Poll::Ready(outcome) = this.block_import.poll(cx) {
            this.on_block_import_result(outcome);
        }

        // These loops drive the entire state of network and does a lot of work. Under heavy load
        // (many messages/events), data may arrive faster than it can be processed (incoming
        // messages/requests -> events), and it is possible that more data has already arrived by
        // the time an internal event is processed. Which could turn this loop into a busy loop.
        // Without yielding back to the executor, it can starve other tasks waiting on that
        // executor to execute them, or drive underlying resources To prevent this, we
        // preemptively return control when the `budget` is exhausted. The value itself is chosen
        // somewhat arbitrarily, it is high enough so the swarm can make meaningful progress but
        // low enough that this loop does not starve other tasks for too long. If the budget is
        // exhausted we manually yield back control to the (coop) scheduler. This manual yield
        // point should prevent situations where polling appears to be frozen. See also
        // <https://tokio.rs/blog/2020-04-preemption> And tokio's docs on cooperative scheduling
        // <https://docs.rs/tokio/latest/tokio/task/#cooperative-scheduling>
        //
        // Testing has shown that this loop naturally reaches the pending state within 1-5
        // iterations in << 100µs in most cases. On average it requires ~50µs, which is inside the
        // range of what's recommended as rule of thumb.
        // <https://ryhl.io/blog/async-what-is-blocking/>

        // process incoming messages from a handle (`TransactionsManager` has one)
        //
        // will only be closed if the channel was deliberately closed since we always have an
        // instance of `NetworkHandle`
        let start_network_handle = Instant::now();
        let maybe_more_handle_messages = poll_nested_stream_with_budget!(
            "net",
            "Network message channel",
            DEFAULT_BUDGET_TRY_DRAIN_NETWORK_HANDLE_CHANNEL,
            this.from_handle_rx.poll_next_unpin(cx),
            |msg| this.on_handle_message(msg),
            error!("Network channel closed");
        );
        poll_durations.acc_network_handle = start_network_handle.elapsed();

        // process incoming messages from the network
        let maybe_more_swarm_events = poll_nested_stream_with_budget!(
            "net",
            "Swarm events stream",
            DEFAULT_BUDGET_TRY_DRAIN_SWARM,
            this.swarm.poll_next_unpin(cx),
            |event| this.on_swarm_event(event),
        );
        poll_durations.acc_swarm =
            start_network_handle.elapsed() - poll_durations.acc_network_handle;

        // all streams are fully drained and import futures pending
        if maybe_more_handle_messages || maybe_more_swarm_events {
            // make sure we're woken up again
            cx.waker().wake_by_ref();
            return Poll::Pending
        }

        this.update_poll_metrics(start, poll_durations);

        Poll::Pending
    }
}

#[derive(Debug, Default)]
struct NetworkManagerPollDurations {
    acc_network_handle: Duration,
    acc_swarm: Duration,
}

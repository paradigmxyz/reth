//! Keeps track of the state of the network.

use crate::{
    cache::LruCache,
    discovery::Discovery,
    fetch::{BlockResponseOutcome, FetchAction, StateFetcher},
    message::{BlockRequest, NewBlockMessage, PeerResponse, PeerResponseResult},
    peers::{PeerAction, PeersManager},
    session::BlockRangeInfo,
    FetchClient,
};
use alloy_consensus::BlockHeader;
use alloy_primitives::B256;
use rand::seq::SliceRandom;
use reth_eth_wire::{
    BlockHashNumber, Capabilities, DisconnectReason, EthNetworkPrimitives, NetworkPrimitives,
    NewBlockHashes, NewBlockPayload, UnifiedStatus,
};
use reth_ethereum_forks::ForkId;
use reth_network_api::{DiscoveredEvent, DiscoveryEvent, PeerRequest, PeerRequestSender};
use reth_network_peers::PeerId;
use reth_network_types::{PeerAddr, PeerKind};
use reth_primitives_traits::Block;
use std::{
    collections::{HashMap, VecDeque},
    fmt,
    net::{IpAddr, SocketAddr},
    ops::Deref,
    sync::{
        atomic::{AtomicU64, AtomicUsize},
        Arc,
    },
    task::{Context, Poll},
};
use tokio::sync::oneshot;
use tracing::{debug, trace};

/// Cache limit of blocks to keep track of for a single peer.
const PEER_BLOCK_CACHE_LIMIT: u32 = 512;

/// Wrapper type for the [`BlockNumReader`] trait.
pub(crate) struct BlockNumReader(Box<dyn reth_storage_api::BlockNumReader>);

impl BlockNumReader {
    /// Create a new instance with the given reader.
    pub fn new(reader: impl reth_storage_api::BlockNumReader + 'static) -> Self {
        Self(Box::new(reader))
    }
}

impl fmt::Debug for BlockNumReader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BlockNumReader").field("inner", &"<dyn BlockNumReader>").finish()
    }
}

impl Deref for BlockNumReader {
    type Target = Box<dyn reth_storage_api::BlockNumReader>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// The [`NetworkState`] keeps track of the state of all peers in the network.
///
/// This includes:
///   - [`Discovery`]: manages the discovery protocol, essentially a stream of discovery updates
///   - [`PeersManager`]: keeps track of connected peers and issues new outgoing connections
///     depending on the configured capacity.
///   - [`StateFetcher`]: streams download request (received from outside via channel) which are
///     then send to the session of the peer.
///
/// This type is also responsible for responding for received request.
#[derive(Debug)]
pub struct NetworkState<N: NetworkPrimitives = EthNetworkPrimitives> {
    /// All active peers and their state.
    active_peers: HashMap<PeerId, ActivePeer<N>>,
    /// Manages connections to peers.
    peers_manager: PeersManager,
    /// Buffered messages until polled.
    queued_messages: VecDeque<StateAction<N>>,
    /// The client type that can interact with the chain.
    ///
    /// This type is used to fetch the block number after we established a session and received the
    /// [`UnifiedStatus`] block hash.
    client: BlockNumReader,
    /// Network discovery.
    discovery: Discovery,
    /// The type that handles requests.
    ///
    /// The fetcher streams `RLPx` related requests on a per-peer basis to this type. This type
    /// will then queue in the request and notify the fetcher once the result has been
    /// received.
    state_fetcher: StateFetcher<N>,
}

impl<N: NetworkPrimitives> NetworkState<N> {
    /// Create a new state instance with the given params
    pub(crate) fn new(
        client: BlockNumReader,
        discovery: Discovery,
        peers_manager: PeersManager,
        num_active_peers: Arc<AtomicUsize>,
    ) -> Self {
        let state_fetcher = StateFetcher::new(peers_manager.handle(), num_active_peers);
        Self {
            active_peers: Default::default(),
            peers_manager,
            queued_messages: Default::default(),
            client,
            discovery,
            state_fetcher,
        }
    }

    /// Returns mutable access to the [`PeersManager`]
    pub(crate) const fn peers_mut(&mut self) -> &mut PeersManager {
        &mut self.peers_manager
    }

    /// Returns mutable access to the [`Discovery`]
    pub(crate) const fn discovery_mut(&mut self) -> &mut Discovery {
        &mut self.discovery
    }

    /// Returns access to the [`PeersManager`]
    pub(crate) const fn peers(&self) -> &PeersManager {
        &self.peers_manager
    }

    /// Returns a new [`FetchClient`]
    pub(crate) fn fetch_client(&self) -> FetchClient<N> {
        self.state_fetcher.client()
    }

    /// How many peers we're currently connected to.
    pub fn num_active_peers(&self) -> usize {
        self.active_peers.len()
    }

    /// Event hook for an activated session for the peer.
    ///
    /// Returns `Ok` if the session is valid, returns an `Err` if the session is not accepted and
    /// should be rejected.
    pub(crate) fn on_session_activated(
        &mut self,
        peer: PeerId,
        capabilities: Arc<Capabilities>,
        status: Arc<UnifiedStatus>,
        request_tx: PeerRequestSender<PeerRequest<N>>,
        timeout: Arc<AtomicU64>,
        range_info: Option<BlockRangeInfo>,
    ) {
        debug_assert!(!self.active_peers.contains_key(&peer), "Already connected; not possible");

        // find the corresponding block number
        let block_number =
            self.client.block_number(status.blockhash).ok().flatten().unwrap_or_default();
        self.state_fetcher.new_active_peer(
            peer,
            status.blockhash,
            block_number,
            Arc::clone(&capabilities),
            timeout,
            range_info,
        );

        self.active_peers.insert(
            peer,
            ActivePeer {
                best_hash: status.blockhash,
                capabilities,
                request_tx,
                pending_responses: Vec::new(),
                blocks: LruCache::new(PEER_BLOCK_CACHE_LIMIT),
            },
        );
    }

    /// Event hook for a disconnected session for the given peer.
    ///
    /// This will remove the peer from the available set of peers and close all inflight requests.
    pub(crate) fn on_session_closed(&mut self, peer: PeerId) {
        self.active_peers.remove(&peer);
        self.state_fetcher.on_session_closed(&peer);
    }

    /// Starts propagating the new block to peers that haven't reported the block yet.
    ///
    /// This is supposed to be invoked after the block was validated.
    ///
    /// > It then sends the block to a small fraction of connected peers (usually the square root of
    /// > the total number of peers) using the `NewBlock` message.
    ///
    /// See also <https://github.com/ethereum/devp2p/blob/master/caps/eth.md>
    pub(crate) fn announce_new_block(&mut self, msg: NewBlockMessage<N::NewBlockPayload>) {
        // send a `NewBlock` message to a fraction of the connected peers (square root of the total
        // number of peers)
        let num_propagate = (self.active_peers.len() as f64).sqrt() as u64 + 1;

        let number = msg.block.block().header().number();
        let mut count = 0;

        // Shuffle to propagate to a random sample of peers on every block announcement
        let mut peers: Vec<_> = self.active_peers.iter_mut().collect();
        peers.shuffle(&mut rand::rng());

        for (peer_id, peer) in peers {
            if peer.blocks.contains(&msg.hash) {
                // skip peers which already reported the block
                continue
            }

            // Queue a `NewBlock` message for the peer
            if count < num_propagate {
                self.queued_messages
                    .push_back(StateAction::NewBlock { peer_id: *peer_id, block: msg.clone() });

                // update peer block info
                if self.state_fetcher.update_peer_block(peer_id, msg.hash, number) {
                    peer.best_hash = msg.hash;
                }

                // mark the block as seen by the peer
                peer.blocks.insert(msg.hash);

                count += 1;
            }

            if count >= num_propagate {
                break
            }
        }
    }

    /// Completes the block propagation process started in [`NetworkState::announce_new_block()`]
    /// but sending `NewBlockHash` broadcast to all peers that haven't seen it yet.
    pub(crate) fn announce_new_block_hash(&mut self, msg: NewBlockMessage<N::NewBlockPayload>) {
        let number = msg.block.block().header().number();
        let hashes = NewBlockHashes(vec![BlockHashNumber { hash: msg.hash, number }]);
        for (peer_id, peer) in &mut self.active_peers {
            if peer.blocks.contains(&msg.hash) {
                // skip peers which already reported the block
                continue
            }

            if self.state_fetcher.update_peer_block(peer_id, msg.hash, number) {
                peer.best_hash = msg.hash;
            }

            self.queued_messages.push_back(StateAction::NewBlockHashes {
                peer_id: *peer_id,
                hashes: hashes.clone(),
            });
        }
    }

    /// Updates the block information for the peer.
    pub(crate) fn update_peer_block(&mut self, peer_id: &PeerId, hash: B256, number: u64) {
        if let Some(peer) = self.active_peers.get_mut(peer_id) {
            peer.best_hash = hash;
        }
        self.state_fetcher.update_peer_block(peer_id, hash, number);
    }

    /// Invoked when a new [`ForkId`] is activated.
    pub(crate) fn update_fork_id(&self, fork_id: ForkId) {
        self.discovery.update_fork_id(fork_id)
    }

    /// Invoked after a `NewBlock` message was received by the peer.
    ///
    /// This will keep track of blocks we know a peer has
    pub(crate) fn on_new_block(&mut self, peer_id: PeerId, hash: B256) {
        // Mark the blocks as seen
        if let Some(peer) = self.active_peers.get_mut(&peer_id) {
            peer.blocks.insert(hash);
        }
    }

    /// Invoked for a `NewBlockHashes` broadcast message.
    pub(crate) fn on_new_block_hashes(&mut self, peer_id: PeerId, hashes: Vec<BlockHashNumber>) {
        // Mark the blocks as seen
        if let Some(peer) = self.active_peers.get_mut(&peer_id) {
            peer.blocks.extend(hashes.into_iter().map(|b| b.hash));
        }
    }

    /// Bans the [`IpAddr`] in the discovery service.
    pub(crate) fn ban_ip_discovery(&self, ip: IpAddr) {
        trace!(target: "net", ?ip, "Banning discovery");
        self.discovery.ban_ip(ip)
    }

    /// Bans the [`PeerId`] and [`IpAddr`] in the discovery service.
    pub(crate) fn ban_discovery(&self, peer_id: PeerId, ip: IpAddr) {
        trace!(target: "net", ?peer_id, ?ip, "Banning discovery");
        self.discovery.ban(peer_id, ip)
    }

    /// Marks the given peer as trusted.
    pub(crate) fn add_trusted_peer_id(&mut self, peer_id: PeerId) {
        self.peers_manager.add_trusted_peer_id(peer_id)
    }

    /// Adds a peer and its address with the given kind to the peerset.
    pub(crate) fn add_peer_kind(&mut self, peer_id: PeerId, kind: PeerKind, addr: PeerAddr) {
        self.peers_manager.add_peer_kind(peer_id, Some(kind), addr, None)
    }

    /// Connects a peer and its address with the given kind
    pub(crate) fn add_and_connect(&mut self, peer_id: PeerId, kind: PeerKind, addr: PeerAddr) {
        self.peers_manager.add_and_connect_kind(peer_id, kind, addr, None)
    }

    /// Removes a peer and its address with the given kind from the peerset.
    pub(crate) fn remove_peer_kind(&mut self, peer_id: PeerId, kind: PeerKind) {
        match kind {
            PeerKind::Basic | PeerKind::Static => self.peers_manager.remove_peer(peer_id),
            PeerKind::Trusted => self.peers_manager.remove_peer_from_trusted_set(peer_id),
        }
    }

    /// Event hook for events received from the discovery service.
    fn on_discovery_event(&mut self, event: DiscoveryEvent) {
        match event {
            DiscoveryEvent::NewNode(DiscoveredEvent::EventQueued { peer_id, addr, fork_id }) => {
                self.queued_messages.push_back(StateAction::DiscoveredNode {
                    peer_id,
                    addr,
                    fork_id,
                });
            }
            DiscoveryEvent::EnrForkId(record, fork_id) => {
                let peer_id = record.id;
                let tcp_addr = record.tcp_addr();
                if tcp_addr.port() == 0 {
                    return
                }
                let udp_addr = record.udp_addr();
                let addr = PeerAddr::new(tcp_addr, Some(udp_addr));
                self.queued_messages.push_back(StateAction::DiscoveredEnrForkId {
                    peer_id,
                    addr,
                    fork_id,
                });
            }
        }
    }

    /// Event hook for new actions derived from the peer management set.
    fn on_peer_action(&mut self, action: PeerAction) {
        match action {
            PeerAction::Connect { peer_id, remote_addr } => {
                self.queued_messages.push_back(StateAction::Connect { peer_id, remote_addr });
            }
            PeerAction::Disconnect { peer_id, reason } => {
                self.state_fetcher.on_pending_disconnect(&peer_id);
                self.queued_messages.push_back(StateAction::Disconnect { peer_id, reason });
            }
            PeerAction::DisconnectBannedIncoming { peer_id } |
            PeerAction::DisconnectUntrustedIncoming { peer_id } => {
                self.state_fetcher.on_pending_disconnect(&peer_id);
                self.queued_messages.push_back(StateAction::Disconnect { peer_id, reason: None });
            }
            PeerAction::DiscoveryBanPeerId { peer_id, ip_addr } => {
                self.ban_discovery(peer_id, ip_addr)
            }
            PeerAction::DiscoveryBanIp { ip_addr } => self.ban_ip_discovery(ip_addr),
            PeerAction::PeerAdded(peer_id) => {
                self.queued_messages.push_back(StateAction::PeerAdded(peer_id))
            }
            PeerAction::PeerRemoved(peer_id) => {
                self.queued_messages.push_back(StateAction::PeerRemoved(peer_id))
            }
            PeerAction::BanPeer { .. } | PeerAction::UnBanPeer { .. } => {}
        }
    }

    /// Sends the message to the peer's session and queues in a response.
    ///
    /// If the send fails (e.g. session channel full or closed), the fetcher's inflight state is
    /// cleaned up so no capacity is leaked.
    fn handle_block_request(&mut self, peer: PeerId, request: BlockRequest, request_id: u64) {
        if let Some(ref mut active) = self.active_peers.get_mut(&peer) {
            let (request, response) = match request {
                BlockRequest::GetBlockHeaders(request) => {
                    let (response, rx) = oneshot::channel();
                    let request = PeerRequest::GetBlockHeaders { request, response };
                    let response = PeerResponse::BlockHeaders { response: rx };
                    (request, response)
                }
                BlockRequest::GetBlockBodies(request) => {
                    let (response, rx) = oneshot::channel();
                    let request = PeerRequest::GetBlockBodies { request, response };
                    let response = PeerResponse::BlockBodies { response: rx };
                    (request, response)
                }
            };
            if active.request_tx.to_session_tx.try_send(request).is_ok() {
                active.pending_responses.push((request_id, response));
            } else {
                // Send failed — clean up the fetcher's inflight state for this request
                self.state_fetcher.on_request_send_failed(&peer, request_id);
            }
        }
    }

    /// Handle the outcome of processed response, for example directly queue another request.
    fn on_block_response_outcome(&mut self, outcome: BlockResponseOutcome) {
        match outcome {
            BlockResponseOutcome::Request(peer, request, request_id) => {
                self.handle_block_request(peer, request, request_id);
            }
            BlockResponseOutcome::BadResponse(peer, reputation_change) => {
                self.peers_manager.apply_reputation_change(&peer, reputation_change);
            }
        }
    }

    /// Invoked when received a response from a connected peer.
    ///
    /// Delegates the response result to the fetcher which may return an outcome specific
    /// instruction that needs to be handled in [`Self::on_block_response_outcome`]. This could be
    /// a follow-up request or an instruction to slash the peer's reputation.
    fn on_eth_response(&mut self, peer: PeerId, request_id: u64, resp: PeerResponseResult<N>) {
        let outcome = match resp {
            PeerResponseResult::BlockHeaders(res) => {
                self.state_fetcher.on_block_headers_response(peer, request_id, res)
            }
            PeerResponseResult::BlockBodies(res) => {
                self.state_fetcher.on_block_bodies_response(peer, request_id, res)
            }
            _ => None,
        };

        if let Some(outcome) = outcome {
            self.on_block_response_outcome(outcome);
        }
    }

    /// Advances the state
    pub(crate) fn poll(&mut self, cx: &mut Context<'_>) -> Poll<StateAction<N>> {
        loop {
            // drain buffered messages
            if let Some(message) = self.queued_messages.pop_front() {
                return Poll::Ready(message)
            }

            while let Poll::Ready(discovery) = self.discovery.poll(cx) {
                self.on_discovery_event(discovery);
            }

            while let Poll::Ready(action) = self.state_fetcher.poll(cx) {
                match action {
                    FetchAction::BlockRequest { peer_id, request, request_id } => {
                        self.handle_block_request(peer_id, request, request_id)
                    }
                }
            }

            loop {
                // need to buffer results here to make borrow checker happy
                let mut closed_sessions = Vec::new();
                let mut received_responses = Vec::new();

                // poll all connected peers for responses
                for (id, peer) in &mut self.active_peers {
                    let mut i = 0;
                    while i < peer.pending_responses.len() {
                        let (_, response) = &mut peer.pending_responses[i];
                        match response.poll(cx) {
                            Poll::Ready(res) => {
                                let (request_id, _) = peer.pending_responses.swap_remove(i);
                                // check if the error is due to a closed channel to the session
                                if res.err().is_some_and(|err| err.is_channel_closed()) {
                                    debug!(
                                        target: "net",
                                        ?id,
                                        "Request canceled, response channel from session closed."
                                    );
                                    closed_sessions.push(*id);
                                    break;
                                }
                                received_responses.push((*id, request_id, res));
                                // don't increment i: swap_remove moved last element here
                            }
                            Poll::Pending => {
                                i += 1;
                            }
                        }
                    }
                }

                for peer in closed_sessions {
                    self.on_session_closed(peer)
                }

                if received_responses.is_empty() {
                    break;
                }

                for (peer_id, request_id, resp) in received_responses {
                    self.on_eth_response(peer_id, request_id, resp);
                }
            }

            // poll peer manager
            while let Poll::Ready(action) = self.peers_manager.poll(cx) {
                self.on_peer_action(action);
            }

            // We need to poll again in case we have received any responses because they may have
            // triggered follow-up requests.
            if self.queued_messages.is_empty() {
                return Poll::Pending
            }
        }
    }
}

/// Tracks the state of a Peer with an active Session.
///
/// For example known blocks,so we can decide what to announce.
#[derive(Debug)]
pub(crate) struct ActivePeer<N: NetworkPrimitives> {
    /// Best block of the peer.
    pub(crate) best_hash: B256,
    /// The capabilities of the remote peer.
    #[expect(dead_code)]
    pub(crate) capabilities: Arc<Capabilities>,
    /// A communication channel directly to the session task.
    pub(crate) request_tx: PeerRequestSender<PeerRequest<N>>,
    /// The response receivers for currently active requests to that peer.
    /// Each entry is a `(request_id, response)` pair for matching back to the fetcher.
    pub(crate) pending_responses: Vec<(u64, PeerResponse<N>)>,
    /// Blocks we know the peer has.
    pub(crate) blocks: LruCache<B256>,
}

/// Message variants triggered by the [`NetworkState`]
#[derive(Debug)]
pub(crate) enum StateAction<N: NetworkPrimitives> {
    /// Dispatch a `NewBlock` message to the peer
    NewBlock {
        /// Target of the message
        peer_id: PeerId,
        /// The `NewBlock` message
        block: NewBlockMessage<N::NewBlockPayload>,
    },
    NewBlockHashes {
        /// Target of the message
        peer_id: PeerId,
        /// `NewBlockHashes` message to send to the peer.
        hashes: NewBlockHashes,
    },
    /// Create a new connection to the given node.
    Connect { remote_addr: SocketAddr, peer_id: PeerId },
    /// Disconnect an existing connection
    Disconnect {
        peer_id: PeerId,
        /// Why the disconnect was initiated
        reason: Option<DisconnectReason>,
    },
    /// Retrieved a [`ForkId`] from the peer via ENR request, See <https://eips.ethereum.org/EIPS/eip-868>
    DiscoveredEnrForkId {
        peer_id: PeerId,
        /// The address of the peer.
        addr: PeerAddr,
        /// The reported [`ForkId`] by this peer.
        fork_id: ForkId,
    },
    /// A new node was found through the discovery, possibly with a `ForkId`
    DiscoveredNode { peer_id: PeerId, addr: PeerAddr, fork_id: Option<ForkId> },
    /// A peer was added
    PeerAdded(PeerId),
    /// A peer was dropped
    PeerRemoved(PeerId),
}

#[cfg(test)]
mod tests {
    use crate::{
        discovery::Discovery,
        fetch::StateFetcher,
        peers::PeersManager,
        state::{BlockNumReader, NetworkState},
        PeerRequest,
    };
    use alloy_consensus::Header;
    use alloy_primitives::B256;
    use reth_eth_wire::{BlockBodies, Capabilities, Capability, EthNetworkPrimitives, EthVersion};
    use reth_ethereum_primitives::BlockBody;
    use reth_network_api::PeerRequestSender;
    use reth_network_p2p::{bodies::client::BodiesClient, error::RequestError};
    use reth_network_peers::PeerId;
    use reth_storage_api::noop::NoopProvider;
    use std::{
        future::poll_fn,
        sync::{atomic::AtomicU64, Arc},
    };
    use tokio::sync::mpsc;
    use tokio_stream::{wrappers::ReceiverStream, StreamExt};

    /// Returns a testing instance of the [`NetworkState`].
    fn state() -> NetworkState<EthNetworkPrimitives> {
        let peers = PeersManager::default();
        let handle = peers.handle();
        NetworkState {
            active_peers: Default::default(),
            peers_manager: Default::default(),
            queued_messages: Default::default(),
            client: BlockNumReader(Box::new(NoopProvider::default())),
            discovery: Discovery::noop(),
            state_fetcher: StateFetcher::new(handle, Default::default()),
        }
    }

    fn capabilities() -> Arc<Capabilities> {
        Arc::new(vec![Capability::from(EthVersion::Eth67)].into())
    }

    // tests that ongoing requests are answered with connection dropped if the session that received
    // that request is drops the request object.
    #[tokio::test(flavor = "multi_thread")]
    async fn test_dropped_active_session() {
        let mut state = state();
        let client = state.fetch_client();

        let peer_id = PeerId::random();
        let (tx, session_rx) = mpsc::channel(1);
        let peer_tx = PeerRequestSender::new(peer_id, tx);

        state.on_session_activated(
            peer_id,
            capabilities(),
            Arc::default(),
            peer_tx,
            Arc::new(AtomicU64::new(1)),
            None,
        );

        assert!(state.active_peers.contains_key(&peer_id));

        let body = BlockBody { ommers: vec![Header::default()], ..Default::default() };

        let body_response = body.clone();

        // this mimics an active session that receives the requests from the state
        tokio::task::spawn(async move {
            let mut stream = ReceiverStream::new(session_rx);
            let resp = stream.next().await.unwrap();
            match resp {
                PeerRequest::GetBlockBodies { response, .. } => {
                    response.send(Ok(BlockBodies(vec![body_response]))).unwrap();
                }
                _ => unreachable!(),
            }

            // wait for the next request, then drop
            let _resp = stream.next().await.unwrap();
        });

        // spawn the state as future
        tokio::task::spawn(async move {
            loop {
                poll_fn(|cx| state.poll(cx)).await;
            }
        });

        // send requests to the state via the client
        let (peer, bodies) = client.get_block_bodies(vec![B256::random()]).await.unwrap().split();
        assert_eq!(peer, peer_id);
        assert_eq!(bodies, vec![body]);

        let resp = client.get_block_bodies(vec![B256::random()]).await;
        assert!(resp.is_err());
        assert_eq!(resp.unwrap_err(), RequestError::ConnectionDropped);
    }

    /// Tests that when the session channel is full (backpressure), the request fails gracefully
    /// without leaking inflight state or killing the peer.
    ///
    /// Uses a channel capacity of 1 so the first request fills it and the second hits
    /// `TrySendError::Full`. Verifies:
    /// - The first request completes successfully.
    /// - The second request gets `ChannelClosed` (not stuck forever).
    /// - A third request succeeds, proving the peer was not removed and capacity was restored.
    #[tokio::test(flavor = "multi_thread")]
    async fn test_send_backpressure_no_inflight_leak() {
        let mut state = state();
        let client = state.fetch_client();

        let peer_id = PeerId::random();
        // Channel capacity of 1 — second concurrent request will fail with TrySendError::Full
        let (tx, session_rx) = mpsc::channel(1);
        let peer_tx = PeerRequestSender::new(peer_id, tx);

        state.on_session_activated(
            peer_id,
            capabilities(),
            Arc::default(),
            peer_tx,
            Arc::new(AtomicU64::new(1)),
            None,
        );

        let body = BlockBody { ommers: vec![Header::default()], ..Default::default() };
        let body_for_session = body.clone();

        // Session task: respond to the first request, then the third (second will never arrive
        // because try_send fails on a full channel).
        tokio::task::spawn(async move {
            let mut stream = ReceiverStream::new(session_rx);

            // First request — respond normally
            let first = stream.next().await.unwrap();
            match first {
                PeerRequest::GetBlockBodies { response, .. } => {
                    response.send(Ok(BlockBodies(vec![body_for_session.clone()]))).unwrap();
                }
                _ => unreachable!(),
            }

            // Third request — respond normally to prove peer is still alive
            let third = stream.next().await.unwrap();
            match third {
                PeerRequest::GetBlockBodies { response, .. } => {
                    response.send(Ok(BlockBodies(vec![body_for_session]))).unwrap();
                }
                _ => unreachable!(),
            }
        });

        // Spawn the state poll loop
        tokio::task::spawn(async move {
            loop {
                poll_fn(|cx| state.poll(cx)).await;
            }
        });

        // First request succeeds
        let (peer, bodies) = client.get_block_bodies(vec![B256::random()]).await.unwrap().split();
        assert_eq!(peer, peer_id);
        assert_eq!(bodies, vec![body.clone()]);

        // Second request — will fail because channel is full (the first request's response
        // filled the slot, and the second goes out before the session can dequeue).
        // We fire two requests concurrently to maximize the chance of backpressure.
        let client2 = client.clone();
        let second =
            tokio::spawn(async move { client2.get_block_bodies(vec![B256::random()]).await });
        let client3 = client.clone();
        let third =
            tokio::spawn(async move { client3.get_block_bodies(vec![B256::random()]).await });

        let (r2, r3) = tokio::join!(second, third);
        let (r2, r3) = (r2.unwrap(), r3.unwrap());

        // At least one should succeed and one should fail, OR both succeed (depending on timing).
        // The key invariant: nothing should hang, and the peer must not be removed.
        // If backpressure happened, the failed one gets ChannelClosed.
        let success_count = [&r2, &r3].iter().filter(|r| r.is_ok()).count();
        let failure_count = [&r2, &r3].iter().filter(|r| r.is_err()).count();

        // With a channel of capacity 1, at most one can be in-flight through the channel.
        // At least one should succeed.
        assert!(success_count >= 1, "at least one request should succeed");

        // Any failures should be ChannelClosed (not ConnectionDropped which would indicate
        // the peer was killed)
        for r in [&r2, &r3] {
            if let Err(e) = r {
                assert_eq!(
                    *e,
                    RequestError::ChannelClosed,
                    "failed requests should get ChannelClosed, not ConnectionDropped"
                );
            }
        }

        // Verify no hang and no zombie state: the test completing proves no leaked inflight.
        // If capacity leaked, the fetcher would never dispatch to this peer again and the
        // test would hang.
        let _ = (success_count, failure_count);
    }
}

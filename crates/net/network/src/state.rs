//! Keeps track of the state of the network.

use crate::{
    cache::LruCache,
    discovery::{Discovery, DiscoveryEvent},
    fetch::{BlockResponseOutcome, FetchAction, StateFetcher, StatusUpdate},
    message::{
        BlockRequest, NewBlockMessage, PeerRequest, PeerRequestSender, PeerResponse,
        PeerResponseResult,
    },
    peers::{PeerAction, PeersManager},
    FetchClient,
};
use reth_eth_wire::{
    capability::Capabilities, BlockHashNumber, DisconnectReason, NewBlockHashes, Status,
};
use reth_interfaces::provider::BlockProvider;
use reth_primitives::{PeerId, H256};
use std::{
    collections::{HashMap, VecDeque},
    net::SocketAddr,
    num::NonZeroUsize,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::sync::oneshot;
use tracing::trace;

/// Cache limit of blocks to keep track of for a single peer.
const PEER_BLOCK_CACHE_LIMIT: usize = 512;

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
pub struct NetworkState<C> {
    /// All connected peers and their state.
    connected_peers: HashMap<PeerId, ConnectedPeer>,
    /// Manages connections to peers.
    peers_manager: PeersManager,
    /// Buffered messages until polled.
    queued_messages: VecDeque<StateAction>,
    /// The client type that can interact with the chain.
    client: Arc<C>,
    /// Network discovery.
    discovery: Discovery,
    /// The genesis hash of the network we're on
    genesis_hash: H256,
    /// The type that handles requests.
    ///
    /// The fetcher streams RLPx related requests on a per-peer basis to this type. This type will
    /// then queue in the request and notify the fetcher once the result has been received.
    state_fetcher: StateFetcher,
}

impl<C> NetworkState<C>
where
    C: BlockProvider,
{
    /// Create a new state instance with the given params
    pub(crate) fn new(
        client: Arc<C>,
        discovery: Discovery,
        peers_manager: PeersManager,
        genesis_hash: H256,
    ) -> Self {
        Self {
            connected_peers: Default::default(),
            peers_manager,
            queued_messages: Default::default(),
            client,
            discovery,
            genesis_hash,
            state_fetcher: Default::default(),
        }
    }

    /// Returns mutable access to the [`PeersManager`]
    pub(crate) fn peers_mut(&mut self) -> &mut PeersManager {
        &mut self.peers_manager
    }

    /// Returns access to the [`PeersManager`]
    pub(crate) fn peers(&self) -> &PeersManager {
        &self.peers_manager
    }

    /// Returns a new [`FetchClient`]
    pub(crate) fn fetch_client(&self) -> FetchClient {
        self.state_fetcher.client()
    }

    /// How many peers we're currently connected to.
    pub fn genesis_hash(&self) -> H256 {
        self.genesis_hash
    }

    /// How many peers we're currently connected to.
    pub fn num_connected_peers(&self) -> usize {
        self.connected_peers.len()
    }

    /// Event hook for an activated session for the peer.
    ///
    /// Returns `Ok` if the session is valid, returns an `Err` if the session is not accepted and
    /// should be rejected.
    pub(crate) fn on_session_activated(
        &mut self,
        peer: PeerId,
        capabilities: Arc<Capabilities>,
        status: Status,
        request_tx: PeerRequestSender,
    ) {
        debug_assert!(!self.connected_peers.contains_key(&peer), "Already connected; not possible");

        // find the corresponding block number
        let block_number =
            self.client.block_number(status.blockhash).ok().flatten().unwrap_or_default();
        self.state_fetcher.new_connected_peer(peer, status.blockhash, block_number);

        self.connected_peers.insert(
            peer,
            ConnectedPeer {
                best_hash: status.blockhash,
                capabilities,
                request_tx,
                pending_response: None,
                blocks: LruCache::new(NonZeroUsize::new(PEER_BLOCK_CACHE_LIMIT).unwrap()),
            },
        );
    }

    /// Event hook for a disconnected session for the peer.
    pub(crate) fn on_session_closed(&mut self, peer: PeerId) {
        self.connected_peers.remove(&peer);
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
    pub(crate) fn announce_new_block(&mut self, msg: NewBlockMessage) {
        // send a `NewBlock` message to a fraction fo the connected peers (square root of the total
        // number of peers)
        let num_propagate = (self.connected_peers.len() as f64).sqrt() as u64 + 1;

        let number = msg.block.block.header.number;
        let mut count = 0;
        for (peer_id, peer) in self.connected_peers.iter_mut() {
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
    pub(crate) fn announce_new_block_hash(&mut self, msg: NewBlockMessage) {
        let number = msg.block.block.header.number;
        let hashes = NewBlockHashes(vec![BlockHashNumber { hash: msg.hash, number }]);
        for (peer_id, peer) in self.connected_peers.iter_mut() {
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
    pub(crate) fn update_peer_block(&mut self, peer_id: &PeerId, hash: H256, number: u64) {
        if let Some(peer) = self.connected_peers.get_mut(peer_id) {
            peer.best_hash = hash;
        }
        self.state_fetcher.update_peer_block(peer_id, hash, number);
    }

    /// Invoked after a `NewBlock` message was received by the peer.
    ///
    /// This will keep track of blocks we know a peer has
    pub(crate) fn on_new_block(&mut self, peer_id: PeerId, hash: H256) {
        // Mark the blocks as seen
        if let Some(peer) = self.connected_peers.get_mut(&peer_id) {
            peer.blocks.insert(hash);
        }
    }

    /// Invoked for a `NewBlockHashes` broadcast message.
    pub(crate) fn on_new_block_hashes(&mut self, peer_id: PeerId, hashes: Vec<BlockHashNumber>) {
        // Mark the blocks as seen
        if let Some(peer) = self.connected_peers.get_mut(&peer_id) {
            peer.blocks.extend(hashes.into_iter().map(|b| b.hash));
        }
    }

    /// Adds a peer and its address to the peerset.
    pub(crate) fn add_peer_address(&mut self, peer_id: PeerId, addr: SocketAddr) {
        self.peers_manager.add_discovered_node(peer_id, addr)
    }

    /// Event hook for events received from the discovery service.
    fn on_discovery_event(&mut self, event: DiscoveryEvent) {
        match event {
            DiscoveryEvent::Discovered(node, addr) => {
                self.peers_manager.add_discovered_node(node, addr);
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
            PeerAction::DisconnectBannedIncoming { peer_id } => {
                // TODO: can IP ban
                self.state_fetcher.on_pending_disconnect(&peer_id);
                self.queued_messages.push_back(StateAction::Disconnect { peer_id, reason: None });
            }
        }
    }

    /// Disconnect the session
    fn on_session_disconnected(&mut self, peer: PeerId) {
        self.connected_peers.remove(&peer);
    }

    /// Sends The message to the peer's session and queues in a response.
    ///
    /// Caution: this will replace an already pending response. It's the responsibility of the
    /// caller to select the peer.
    fn handle_block_request(&mut self, peer: PeerId, request: BlockRequest) {
        if let Some(ref mut peer) = self.connected_peers.get_mut(&peer) {
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
            let _ = peer.request_tx.to_session_tx.try_send(request);
            peer.pending_response = Some(response);
        }
    }

    /// Handle the outcome of processed response, for example directly queue another request.
    fn on_block_response_outcome(&mut self, outcome: BlockResponseOutcome) -> Option<StateAction> {
        match outcome {
            BlockResponseOutcome::Request(peer, request) => {
                self.handle_block_request(peer, request);
            }
            BlockResponseOutcome::BadResponse(peer, reputation_change) => {
                self.peers_manager.apply_reputation_change(&peer, reputation_change);
            }
        }
        None
    }

    /// Invoked when received a response from a connected peer.
    fn on_eth_response(&mut self, peer: PeerId, resp: PeerResponseResult) -> Option<StateAction> {
        match resp {
            PeerResponseResult::BlockHeaders(res) => {
                let outcome = self.state_fetcher.on_block_headers_response(peer, res)?;
                self.on_block_response_outcome(outcome)
            }
            PeerResponseResult::BlockBodies(res) => {
                let outcome = self.state_fetcher.on_block_bodies_response(peer, res)?;
                self.on_block_response_outcome(outcome)
            }
            _ => None,
        }
    }

    /// Advances the state
    pub(crate) fn poll(&mut self, cx: &mut Context<'_>) -> Poll<StateAction> {
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
                    FetchAction::BlockRequest { peer_id, request } => {
                        self.handle_block_request(peer_id, request)
                    }
                    FetchAction::StatusUpdate(status) => {
                        // we want to return this directly
                        return Poll::Ready(StateAction::StatusUpdate(status))
                    }
                }
            }

            // need to buffer results here to make borrow checker happy
            let mut disconnect_sessions = Vec::new();
            let mut received_responses = Vec::new();

            // poll all connected peers for responses
            for (id, peer) in self.connected_peers.iter_mut() {
                if let Some(response) = peer.pending_response.as_mut() {
                    match response.poll(cx) {
                        Poll::Ready(Err(_)) => {
                            trace!(
                                target : "net",
                                ?id,
                                "Request canceled, response channel closed."
                            );
                            disconnect_sessions.push(*id);
                        }
                        Poll::Ready(Ok(resp)) => received_responses.push((*id, resp)),
                        Poll::Pending => continue,
                    };
                }

                // request has either returned a response or was canceled here
                peer.pending_response.take();
            }

            for node in disconnect_sessions {
                self.on_session_disconnected(node)
            }

            for (id, resp) in received_responses {
                if let Some(action) = self.on_eth_response(id, resp) {
                    self.queued_messages.push_back(action);
                }
            }

            // poll peer manager
            while let Poll::Ready(action) = self.peers_manager.poll(cx) {
                self.on_peer_action(action);
            }

            if self.queued_messages.is_empty() {
                return Poll::Pending
            }
        }
    }
}

/// Tracks the state of a Peer.
///
/// For example known blocks,so we can decide what to announce.
pub(crate) struct ConnectedPeer {
    /// Best block of the peer.
    pub(crate) best_hash: H256,
    /// The capabilities of the connected peer.
    #[allow(unused)]
    pub(crate) capabilities: Arc<Capabilities>,
    /// A communication channel directly to the session task.
    pub(crate) request_tx: PeerRequestSender,
    /// The response receiver for a currently active request to that peer.
    pub(crate) pending_response: Option<PeerResponse>,
    /// Blocks we know the peer has.
    pub(crate) blocks: LruCache<H256>,
}

/// Message variants triggered by the [`State`]
pub(crate) enum StateAction {
    /// Received a node status update.
    StatusUpdate(StatusUpdate),
    /// Dispatch a `NewBlock` message to the peer
    NewBlock {
        /// Target of the message
        peer_id: PeerId,
        /// The `NewBlock` message
        block: NewBlockMessage,
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
}

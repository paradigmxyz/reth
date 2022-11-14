//! Keeps track of the state of the network.

use crate::{
    discovery::{Discovery, DiscoveryEvent},
    fetch::StateFetcher,
    message::{PeerRequestSender, PeerResponse},
    peers::{PeerAction, PeersManager},
    NodeId,
};

use reth_eth_wire::{capability::Capabilities, Status};
use reth_interfaces::provider::BlockProvider;
use reth_primitives::H256;
use std::{
    collections::{HashMap, VecDeque},
    net::SocketAddr,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::sync::oneshot;

use crate::{
    fetch::BlockResponseOutcome,
    message::{BlockRequest, PeerRequest, PeerResponseResult},
};
use tracing::trace;

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
    connected_peers: HashMap<NodeId, ConnectedPeer>,
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

    /// Event hook for an activated session for the peer.
    ///
    /// Returns `Ok` if the session is valid, returns an `Err` if the session is not accepted and
    /// should be rejected.
    pub(crate) fn on_session_activated(
        &mut self,
        peer: NodeId,
        capabilities: Arc<Capabilities>,
        status: Status,
        request_tx: PeerRequestSender,
    ) -> Result<(), AddSessionError> {
        // TODO add capacity check
        debug_assert!(self.connected_peers.contains_key(&peer), "Already connected; not possible");

        self.state_fetcher.new_connected_peer(peer, status.blockhash);

        self.connected_peers.insert(
            peer,
            ConnectedPeer {
                best_hash: status.blockhash,
                capabilities,
                request_tx,
                pending_response: None,
            },
        );

        Ok(())
    }

    /// Event hook for a disconnected session for the peer.
    pub(crate) fn on_session_closed(&mut self, peer: NodeId) {
        self.connected_peers.remove(&peer);
        self.state_fetcher.on_session_closed(&peer);
    }

    /// Propagates Block to peers.
    pub(crate) fn announce_block(&mut self, _hash: H256, _block: ()) {
        // TODO propagate the newblock messages to all connected peers that haven't seen the block
        // yet

        todo!()
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
                self.queued_messages
                    .push_back(StateAction::Connect { node_id: peer_id, remote_addr });
            }
            PeerAction::Disconnect { peer_id } => {
                self.state_fetcher.on_pending_disconnect(&peer_id);
                self.queued_messages.push_back(StateAction::Disconnect { node_id: peer_id });
            }
            PeerAction::DisconnectBannedIncoming { peer_id } => {
                // TODO: can IP ban
                self.state_fetcher.on_pending_disconnect(&peer_id);
                self.queued_messages.push_back(StateAction::Disconnect { node_id: peer_id });
            }
        }
    }

    /// Disconnect the session
    fn on_session_disconnected(&mut self, peer: NodeId) {
        self.connected_peers.remove(&peer);
    }

    /// Sends The message to the peer's session and queues in a response.
    ///
    /// Caution: this will replace an already pending response. It's the responsibility of the
    /// caller to select the peer.
    fn handle_block_request(&mut self, peer: NodeId, request: BlockRequest) {
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
            BlockResponseOutcome::BadResponse(_) => {
                // TODO handle reputation change
            }
        }
        None
    }

    /// Invoked when received a response from a connected peer.
    fn on_eth_response(&mut self, peer: NodeId, resp: PeerResponseResult) -> Option<StateAction> {
        match resp {
            PeerResponseResult::BlockHeaders(res) => {
                let outcome = self.state_fetcher.on_block_headers_response(peer, res)?;
                return self.on_block_response_outcome(outcome)
            }
            PeerResponseResult::BlockBodies(res) => {
                let outcome = self.state_fetcher.on_block_bodies_response(peer, res)?;
                return self.on_block_response_outcome(outcome)
            }
            PeerResponseResult::PooledTransactions(_) => {}
            PeerResponseResult::NodeData(_) => {}
            PeerResponseResult::Receipts(_) => {}
        }
        None
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

            let mut disconnect_sessions = Vec::new();
            let mut received_responses = Vec::new();
            // poll all connected peers for responses
            for (id, peer) in self.connected_peers.iter_mut() {
                if let Some(response) = peer.pending_response.as_mut() {
                    match response.poll(cx) {
                        Poll::Ready(Err(_)) => {
                            trace!(
                                ?id,
                                target = "net",
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
pub struct ConnectedPeer {
    /// Best block of the peer.
    pub(crate) best_hash: H256,
    /// The capabilities of the connected peer.
    pub(crate) capabilities: Arc<Capabilities>,
    /// A communication channel directly to the session task.
    pub(crate) request_tx: PeerRequestSender,
    /// The response receiver for a currently active request to that peer.
    pub(crate) pending_response: Option<PeerResponse>,
}

/// Message variants triggered by the [`State`]
pub enum StateAction {
    /// Create a new connection to the given node.
    Connect { remote_addr: SocketAddr, node_id: NodeId },
    /// Disconnect an existing connection
    Disconnect { node_id: NodeId },
}

#[derive(Debug, thiserror::Error)]
pub enum AddSessionError {
    #[error("No capacity for new sessions")]
    AtCapacity {
        /// The peer of the session
        peer: NodeId,
    },
}

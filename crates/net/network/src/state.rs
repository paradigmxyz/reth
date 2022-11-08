//! Keeps track of the state of the network.

use crate::{
    discovery::{Discovery, DiscoveryEvent},
    fetch::StateFetcher,
    message::{EthResponse, PeerRequestSender, PeerResponse},
    peers::{PeerAction, PeersManager},
    NodeId,
};

use reth_eth_wire::{capability::Capabilities, Status};
use reth_interfaces::provider::BlockProvider;
use reth_primitives::{H256, U256};
use std::{
    collections::{HashMap, VecDeque},
    net::SocketAddr,
    sync::Arc,
    task::{Context, Poll},
    time::Instant,
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
    /// Tracks the state of connected peers
    peers_state: HashMap<NodeId, PeerSessionState>,
    /// Buffered messages until polled.
    queued_messages: VecDeque<StateAction>,
    /// The client type that can interact with the chain.
    client: Arc<C>,
    /// Network discovery.
    discovery: Discovery,
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
    pub(crate) fn new(client: Arc<C>, discovery: Discovery, peers_manager: PeersManager) -> Self {
        Self {
            connected_peers: Default::default(),
            peers_manager,
            peers_state: Default::default(),
            queued_messages: Default::default(),
            client,
            discovery,
            state_fetcher: Default::default(),
        }
    }

    /// Event hook for an authenticated session for the peer.
    pub(crate) fn on_session_authenticated(
        &mut self,
        _node_id: NodeId,
        _capabilities: Arc<Capabilities>,
        _status: Status,
        _messages: PeerRequestSender,
    ) {
        // TODO notify fetecher as well
    }

    /// Event hook for a disconnected session for the peer.
    pub(crate) fn on_session_closed(&mut self, _node_id: NodeId) {}

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
            PeerAction::Connect { node_id, remote_addr } => {
                self.peers_state.insert(node_id, PeerSessionState::Connecting);
                self.queued_messages.push_back(StateAction::Connect { node_id, remote_addr });
            }
            PeerAction::Disconnect { node_id } => {
                self.peers_state.remove(&node_id);
                self.queued_messages.push_back(StateAction::Disconnect { node_id });
            }
        }
    }

    /// Disconnect the session
    fn disconnect_session(&mut self, _node: NodeId) {}

    /// Invoked when received a response from a connected peer.
    fn on_eth_response(&mut self, _node: NodeId, _resp: EthResponse) {}

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
                        Poll::Ready(Ok(resp)) => received_responses.push((*id, resp)),
                        Poll::Ready(Err(_)) => {
                            trace!(
                                ?id,
                                target = "net",
                                "Request canceled, response channel closed."
                            );
                            disconnect_sessions.push(*id);
                        }
                        Poll::Pending => continue,
                    };
                }

                // request has either returned a response or was canceled here
                peer.pending_response.take();
            }

            for node in disconnect_sessions {
                self.disconnect_session(node)
            }

            for (id, resp) in received_responses {
                self.on_eth_response(id, resp);
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
    /// Best block number of the peer.
    pub(crate) best_number: U256,
    /// A communication channel directly to the session service.
    pub(crate) message_tx: PeerRequestSender,
    /// The response receiver for a currently active request to that peer.
    pub(crate) pending_response: Option<PeerResponse>,
}

/// Tracks the current state of the peer session
pub enum PeerSessionState {
    /// Starting state for outbound connections.
    ///
    /// This will be triggered by a [`PeerAction::Connect`] action.
    /// The peer will reside in the state until the connection has been authenticated.
    Connecting,
    /// Established connection that hasn't been authenticated yet.
    Incoming {
        /// How long to keep this open.
        until: Instant,
        sender: PeerRequestSender,
    },
    /// Node is connected to the peer and is ready to
    Ready {
        /// Communication channel directly to the session task
        sender: PeerRequestSender,
    },
}

/// Message variants triggered by the [`State`]
pub enum StateAction {
    /// Create a new connection to the given node.
    Connect { remote_addr: SocketAddr, node_id: NodeId },
    /// Disconnect an existing connection
    Disconnect { node_id: NodeId },
}

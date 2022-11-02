//! Keeps track of the state of the network.

use crate::{
    discovery::{Discovery, DiscoveryEvent},
    fetch::StateFetcher,
    message::{Capabilities, CapabilityMessage},
    peers::{PeerAction, PeersManager},
    session::PeerMessageSender,
    NodeId,
};
use reth_eth_wire::{BlockHeaders, GetBlockHeaders};
use reth_interfaces::provider::BlockProvider;
use reth_primitives::{H256, U256};
use std::{
    collections::{HashMap, VecDeque},
    net::SocketAddr,
    sync::Arc,
    task::{Context, Poll},
    time::Instant,
};
use tokio::sync::oneshot;

/// Maintains the state of all peers in the network.
///
/// This determines how to interact with peers.
pub struct NetworkState<C> {
    /// All connected peers and their state.
    connected_peers: HashMap<NodeId, ConnectedPeer>,
    /// Manages connections to peers.
    peers_manager: PeersManager,
    /// Tracks the state of connected peers
    peers_state: HashMap<NodeId, PeerSessionState>,
    /// Buffered messages until polled.
    queued_messages: VecDeque<StateMessage>,
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
            state_fetcher: StateFetcher::new(),
        }
    }

    /// Event hook for an authenticated session for the peer.
    pub fn on_session_authenticated(
        &mut self,
        _node_id: NodeId,
        _capabilities: Arc<Capabilities>,
        _messages: PeerMessageSender,
    ) {
    }

    /// Event hook for a disconnected session for the peer.
    pub(crate) fn on_session_closed(&mut self, _node_id: NodeId) {}

    /// Propagates Block to peers.
    pub fn announce_block(&mut self, _hash: H256, _block: ()) {
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
                self.queued_messages.push_back(StateMessage::Connect { node_id, remote_addr });
            }
            PeerAction::Disconnect { node_id } => {
                self.peers_state.remove(&node_id);
                self.queued_messages.push_back(StateMessage::Disconnect { node_id });
            }
        }
    }

    /// Advances the state
    pub(crate) fn poll(&mut self, cx: &mut Context<'_>) -> Poll<StateMessage> {
        loop {
            // drain buffered messages
            if let Some(message) = self.queued_messages.pop_front() {
                return Poll::Ready(message)
            }

            while let Poll::Ready(discovery) = self.discovery.poll(cx) {
                self.on_discovery_event(discovery);
            }

            // TODO poll all connected peers and handle responses

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
    pub(crate) message_tx: PeerMessageSender,
    /// The response receiver for a currently active request to that peer.
    ///
    /// TODO the `CapabilityMessage` should be an enum with possible variants
    pub(crate) inflight_request: Option<oneshot::Sender<Result<CapabilityMessage, ()>>>,
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
        sender: PeerMessageSender,
    },
    /// Node is connected to the peer and is ready to
    Ready {
        /// Communication channel directly to the session task
        sender: PeerMessageSender,
    },
}

/// Message variants triggered by the [`State`]
pub enum StateMessage {
    /// Create a new connection to the given node.
    Connect { node_id: NodeId, remote_addr: SocketAddr },
    /// Disconnect an existing connection
    Disconnect { node_id: NodeId },
    /// Request Block headers from the peer.
    ///
    /// The response should be sent through the channel.
    GetBlockHeaders {
        peer: NodeId,
        request: GetBlockHeaders,
        /// TODO the `CapabilityMessage` should be an enum with possible variants
        response: oneshot::Sender<BlockHeaders>,
    },
}

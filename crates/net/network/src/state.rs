//! Keeps track of the state of the network.

use crate::{
    capability::{CapabilityMessage, PeerMessageSender},
    peers::PeersManager,
    session::SessionId,
    NodeId,
};
use reth_eth_wire::{BlockHeaders, GetBlockHeaders};
use reth_interfaces::provider::BlockProvider;
use reth_primitives::{H256, U256};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
    time::Instant,
};
use tokio::sync::oneshot;

/// Maintains the state of all peers in the network.
///
/// This determines how to interact with peers.
pub struct NetworkState<C> {
    /// All connected peers and their state.
    peers_info: HashMap<NodeId, PeerInfo>,
    /// Manages connections to peers.
    peers_manager: PeersManager,
    /// Tracks the state of connected peers
    peers_state: HashMap<NodeId, PeerSessionState>,
    /// Buffered messages until polled.
    queued_messages: VecDeque<StateMessage>,
    /// The client type that can interact with the chain.
    client: Arc<C>,
    /// Network discovery.
    discovery: (),
    /// The type that handles sync requests.
    ///
    /// The fetcher streams RLPx related requests on a per-peer basis to this type. This type will
    /// then queue in the request and notify the fetcher once the result has been received.
    sync_fetcher: (),
}

impl<C> NetworkState<C>
where
    C: BlockProvider,
{
    /// Event hook for an authenticated session for the peer.
    pub fn on_session_authenticated(&mut self, _peer: NodeId, _session: SessionId) {}
    /// Event hook for a disconnected session for the peer.
    pub fn on_session_closed(&mut self, _peer: NodeId, _session: SessionId) {}

    /// Handles a received [`CapabilityMessage`] from the peer.
    pub fn on_capability_message(
        &mut self,
        _peer: NodeId,
        _session: SessionId,
        _msg: CapabilityMessage,
    ) {
        // TODO needs capability+state depended decoding and handling of the message.
        // if this is a request
    }

    /// Returns all nodes that we discovered on the network.
    pub fn known_peers(&mut self) -> HashSet<NodeId> {
        todo!()
    }

    /// Propagates Block to peers.
    pub fn announce_block(&mut self, _hash: H256, _block: ()) {
        todo!()
    }
}

/// Tracks the state of a Peer.
///
/// For example known blocks,so we can decide what to announce.
pub struct PeerInfo {
    /// Best block of the peer.
    pub best_hash: H256,
    /// Best block number of the peer.
    pub best_number: U256,
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
    // TODO add request variants, like `GetBlockHeaders` which is then delegated to the peer
    /// Request Block headers from the peer.
    ///
    /// The response should be sent through the channel.
    GetBlockHeaders {
        peer: NodeId,
        request: GetBlockHeaders,
        response: oneshot::Sender<BlockHeaders>,
    },
}

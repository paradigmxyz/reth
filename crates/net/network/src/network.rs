use crate::{
    config::NetworkMode,
    manager::NetworkEvent,
    message::PeerRequest,
    peers::{PeersHandle, ReputationChangeKind},
};
use parking_lot::Mutex;
use reth_eth_wire::{NewBlock, NewPooledTransactionHashes, Transactions};
use reth_primitives::{PeerId, H256};
use std::{
    net::SocketAddr,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
};
use tokio::sync::{mpsc, mpsc::UnboundedSender};
use tokio_stream::wrappers::UnboundedReceiverStream;

/// A _shareable_ network frontend. Used to interact with the network.
///
/// See also [`NetworkManager`](crate::NetworkManager).
#[derive(Clone)]
pub struct NetworkHandle {
    /// The Arc'ed delegate that contains the state.
    inner: Arc<NetworkInner>,
}

// === impl NetworkHandle ===

impl NetworkHandle {
    /// Creates a single new instance.
    pub(crate) fn new(
        num_active_peers: Arc<AtomicUsize>,
        listener_address: Arc<Mutex<SocketAddr>>,
        to_manager_tx: UnboundedSender<NetworkHandleMessage>,
        local_peer_id: PeerId,
        peers: PeersHandle,
        network_mode: NetworkMode,
    ) -> Self {
        let inner = NetworkInner {
            num_active_peers,
            to_manager_tx,
            listener_address,
            local_peer_id,
            peers,
            network_mode,
        };
        Self { inner: Arc::new(inner) }
    }

    /// How many peers we're currently connected to.
    pub fn num_connected_peers(&self) -> usize {
        self.inner.num_active_peers.load(Ordering::Relaxed)
    }

    /// Returns the [`SocketAddr`] that listens for incoming connections.
    pub fn local_addr(&self) -> SocketAddr {
        *self.inner.listener_address.lock()
    }

    /// Returns the [`PeerId`] used in the network.
    pub fn peer_id(&self) -> &PeerId {
        &self.inner.local_peer_id
    }

    fn manager(&self) -> &UnboundedSender<NetworkHandleMessage> {
        &self.inner.to_manager_tx
    }

    /// Creates a new [`NetworkEvent`] listener channel.
    pub fn event_listener(&self) -> UnboundedReceiverStream<NetworkEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        let _ = self.manager().send(NetworkHandleMessage::EventListener(tx));
        UnboundedReceiverStream::new(rx)
    }

    /// Returns the mode of the network, either pow, or pos
    pub fn mode(&self) -> &NetworkMode {
        &self.inner.network_mode
    }

    /// Sends a [`NetworkHandleMessage`] to the manager
    pub(crate) fn send_message(&self, msg: NetworkHandleMessage) {
        let _ = self.inner.to_manager_tx.send(msg);
    }

    /// Sends a message to the [`NetworkManager`] to add a peer to the known set
    pub fn add_peer(&self, peer: PeerId, addr: SocketAddr) {
        let _ = self.inner.to_manager_tx.send(NetworkHandleMessage::AddPeerAddress(peer, addr));
    }

    /// Sends a message to the [`NetworkManager`] to disconnect an existing connection to the given
    /// peer.
    pub fn disconnect_peer(&self, peer: PeerId) {
        self.send_message(NetworkHandleMessage::DisconnectPeer(peer))
    }

    /// Send a reputation change for the given peer.
    pub fn reputation_change(&self, peer_id: PeerId, kind: ReputationChangeKind) {
        self.send_message(NetworkHandleMessage::ReputationChange(peer_id, kind));
    }

    /// Sends a [`PeerRequest`] to the given peer's session.
    pub fn send_request(&self, peer_id: PeerId, request: PeerRequest) {
        self.send_message(NetworkHandleMessage::EthRequest { peer_id, request })
    }
}

struct NetworkInner {
    /// Number of active peer sessions the node's currently handling.
    num_active_peers: Arc<AtomicUsize>,
    /// Sender half of the message channel to the [`NetworkManager`].
    to_manager_tx: UnboundedSender<NetworkHandleMessage>,
    /// The local address that accepts incoming connections.
    listener_address: Arc<Mutex<SocketAddr>>,
    /// The identifier used by this node.
    local_peer_id: PeerId,
    /// Access to the all the nodes
    peers: PeersHandle,
    /// The mode of the network
    network_mode: NetworkMode,
}

/// Internal messages that can be passed to the  [`NetworkManager`](crate::NetworkManager).
#[allow(missing_docs)]
pub(crate) enum NetworkHandleMessage {
    /// Adds an address for a peer.
    AddPeerAddress(PeerId, SocketAddr),
    /// Disconnect a connection to a peer if it exists.
    DisconnectPeer(PeerId),
    /// Add a new listener for [`NetworkEvent`].
    EventListener(UnboundedSender<NetworkEvent>),
    /// Broadcast event to announce a new block to all nodes.
    AnnounceBlock(NewBlock, H256),
    /// Sends the list of transactions to the given peer.
    SendTransaction { peer_id: PeerId, msg: Arc<Transactions> },
    /// Sends the list of transactions hashes to the given peer.
    SendPooledTransactionHashes { peer_id: PeerId, msg: Arc<NewPooledTransactionHashes> },
    /// Send an `eth` protocol request to the peer.
    EthRequest {
        /// The peer to send the request to.
        peer_id: PeerId,
        /// The request to send to the peer's sessions.
        request: PeerRequest,
    },
    /// Apply a reputation change to the given peer.
    ReputationChange(PeerId, ReputationChangeKind),
}

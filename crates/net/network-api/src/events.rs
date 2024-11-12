//! API related to listening for network events.

use std::{fmt, net::SocketAddr, sync::Arc};

use reth_eth_wire_types::{
    message::RequestPair, BlockBodies, BlockHeaders, Capabilities, DisconnectReason, EthMessage,
    EthNetworkPrimitives, EthVersion, GetBlockBodies, GetBlockHeaders, GetNodeData,
    GetPooledTransactions, GetReceipts, NetworkPrimitives, NodeData, PooledTransactions, Receipts,
    Status,
};
use reth_ethereum_forks::ForkId;
use reth_network_p2p::error::{RequestError, RequestResult};
use reth_network_peers::PeerId;
use reth_network_types::PeerAddr;
use reth_tokio_util::EventStream;
use tokio::sync::{mpsc, oneshot};
use tokio_stream::wrappers::UnboundedReceiverStream;

/// Provides event subscription for the network.
#[auto_impl::auto_impl(&, Arc)]
pub trait NetworkEventListenerProvider: Send + Sync {
    /// Creates a new [`NetworkEvent`] listener channel.
    fn event_listener(&self) -> EventStream<NetworkEvent>;
    /// Returns a new [`DiscoveryEvent`] stream.
    ///
    /// This stream yields [`DiscoveryEvent`]s for each peer that is discovered.
    fn discovery_listener(&self) -> UnboundedReceiverStream<DiscoveryEvent>;
}

/// (Non-exhaustive) Events emitted by the network that are of interest for subscribers.
///
/// This includes any event types that may be relevant to tasks, for metrics, keep track of peers
/// etc.
#[derive(Debug)]
pub enum NetworkEvent<R = PeerRequest> {
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
        /// The remote addr of the peer to which a session was established.
        remote_addr: SocketAddr,
        /// The client version of the peer to which a session was established.
        client_version: Arc<str>,
        /// Capabilities the peer announced
        capabilities: Arc<Capabilities>,
        /// A request channel to the session task.
        messages: PeerRequestSender<R>,
        /// The status of the peer to which a session was established.
        status: Arc<Status>,
        /// negotiated eth version of the session
        version: EthVersion,
    },
    /// Event emitted when a new peer is added
    PeerAdded(PeerId),
    /// Event emitted when a new peer is removed
    PeerRemoved(PeerId),
}

impl<R> Clone for NetworkEvent<R> {
    fn clone(&self) -> Self {
        match self {
            Self::SessionClosed { peer_id, reason } => {
                Self::SessionClosed { peer_id: *peer_id, reason: *reason }
            }
            Self::SessionEstablished {
                peer_id,
                remote_addr,
                client_version,
                capabilities,
                messages,
                status,
                version,
            } => Self::SessionEstablished {
                peer_id: *peer_id,
                remote_addr: *remote_addr,
                client_version: client_version.clone(),
                capabilities: capabilities.clone(),
                messages: messages.clone(),
                status: status.clone(),
                version: *version,
            },
            Self::PeerAdded(peer) => Self::PeerAdded(*peer),
            Self::PeerRemoved(peer) => Self::PeerRemoved(*peer),
        }
    }
}

/// Events produced by the `Discovery` manager.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryEvent {
    /// Discovered a node
    NewNode(DiscoveredEvent),
    /// Retrieved a [`ForkId`] from the peer via ENR request, See <https://eips.ethereum.org/EIPS/eip-868>
    EnrForkId(PeerId, ForkId),
}

/// Represents events related to peer discovery in the network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveredEvent {
    /// Indicates that a new peer has been discovered and queued for potential connection.
    ///
    /// This event is generated when the system becomes aware of a new peer
    /// but hasn't yet established a connection.
    ///
    /// # Fields
    ///
    /// * `peer_id` - The unique identifier of the discovered peer.
    /// * `addr` - The network address of the discovered peer.
    /// * `fork_id` - An optional identifier for the fork that this peer is associated with. `None`
    ///   if the peer is not associated with a specific fork.
    EventQueued {
        /// The unique identifier of the discovered peer.
        peer_id: PeerId,
        /// The network address of the discovered peer.
        addr: PeerAddr,
        /// An optional identifier for the fork that this peer is associated with.
        /// `None` if the peer is not associated with a specific fork.
        fork_id: Option<ForkId>,
    },
}

/// Protocol related request messages that expect a response
#[derive(Debug)]
pub enum PeerRequest<N: NetworkPrimitives = EthNetworkPrimitives> {
    /// Requests block headers from the peer.
    ///
    /// The response should be sent through the channel.
    GetBlockHeaders {
        /// The request for block headers.
        request: GetBlockHeaders,
        /// The channel to send the response for block headers.
        response: oneshot::Sender<RequestResult<BlockHeaders<N::BlockHeader>>>,
    },
    /// Requests block bodies from the peer.
    ///
    /// The response should be sent through the channel.
    GetBlockBodies {
        /// The request for block bodies.
        request: GetBlockBodies,
        /// The channel to send the response for block bodies.
        response: oneshot::Sender<RequestResult<BlockBodies<N::BlockBody>>>,
    },
    /// Requests pooled transactions from the peer.
    ///
    /// The response should be sent through the channel.
    GetPooledTransactions {
        /// The request for pooled transactions.
        request: GetPooledTransactions,
        /// The channel to send the response for pooled transactions.
        response: oneshot::Sender<RequestResult<PooledTransactions>>,
    },
    /// Requests `NodeData` from the peer.
    ///
    /// The response should be sent through the channel.
    GetNodeData {
        /// The request for `NodeData`.
        request: GetNodeData,
        /// The channel to send the response for `NodeData`.
        response: oneshot::Sender<RequestResult<NodeData>>,
    },
    /// Requests receipts from the peer.
    ///
    /// The response should be sent through the channel.
    GetReceipts {
        /// The request for receipts.
        request: GetReceipts,
        /// The channel to send the response for receipts.
        response: oneshot::Sender<RequestResult<Receipts>>,
    },
}

// === impl PeerRequest ===

impl<N: NetworkPrimitives> PeerRequest<N> {
    /// Invoked if we received a response which does not match the request
    pub fn send_bad_response(self) {
        self.send_err_response(RequestError::BadResponse)
    }

    /// Send an error back to the receiver.
    pub fn send_err_response(self, err: RequestError) {
        let _ = match self {
            Self::GetBlockHeaders { response, .. } => response.send(Err(err)).ok(),
            Self::GetBlockBodies { response, .. } => response.send(Err(err)).ok(),
            Self::GetPooledTransactions { response, .. } => response.send(Err(err)).ok(),
            Self::GetNodeData { response, .. } => response.send(Err(err)).ok(),
            Self::GetReceipts { response, .. } => response.send(Err(err)).ok(),
        };
    }

    /// Returns the [`EthMessage`] for this type
    pub fn create_request_message(&self, request_id: u64) -> EthMessage<N> {
        match self {
            Self::GetBlockHeaders { request, .. } => {
                EthMessage::GetBlockHeaders(RequestPair { request_id, message: *request })
            }
            Self::GetBlockBodies { request, .. } => {
                EthMessage::GetBlockBodies(RequestPair { request_id, message: request.clone() })
            }
            Self::GetPooledTransactions { request, .. } => {
                EthMessage::GetPooledTransactions(RequestPair {
                    request_id,
                    message: request.clone(),
                })
            }
            Self::GetNodeData { request, .. } => {
                EthMessage::GetNodeData(RequestPair { request_id, message: request.clone() })
            }
            Self::GetReceipts { request, .. } => {
                EthMessage::GetReceipts(RequestPair { request_id, message: request.clone() })
            }
        }
    }

    /// Consumes the type and returns the inner [`GetPooledTransactions`] variant.
    pub fn into_get_pooled_transactions(self) -> Option<GetPooledTransactions> {
        match self {
            Self::GetPooledTransactions { request, .. } => Some(request),
            _ => None,
        }
    }
}

/// A Cloneable connection for sending _requests_ directly to the session of a peer.
pub struct PeerRequestSender<R = PeerRequest> {
    /// id of the remote node.
    pub peer_id: PeerId,
    /// The Sender half connected to a session.
    pub to_session_tx: mpsc::Sender<R>,
}

impl<R> Clone for PeerRequestSender<R> {
    fn clone(&self) -> Self {
        Self { peer_id: self.peer_id, to_session_tx: self.to_session_tx.clone() }
    }
}

// === impl PeerRequestSender ===

impl<R> PeerRequestSender<R> {
    /// Constructs a new sender instance that's wired to a session
    pub const fn new(peer_id: PeerId, to_session_tx: mpsc::Sender<R>) -> Self {
        Self { peer_id, to_session_tx }
    }

    /// Attempts to immediately send a message on this Sender
    pub fn try_send(&self, req: R) -> Result<(), mpsc::error::TrySendError<R>> {
        self.to_session_tx.try_send(req)
    }

    /// Returns the peer id of the remote peer.
    pub const fn peer_id(&self) -> &PeerId {
        &self.peer_id
    }
}

impl<R> fmt::Debug for PeerRequestSender<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PeerRequestSender").field("peer_id", &self.peer_id).finish_non_exhaustive()
    }
}

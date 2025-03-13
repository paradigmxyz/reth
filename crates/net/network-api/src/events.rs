//! API related to listening for network events.

use reth_eth_wire_types::{
    message::RequestPair, BlockBodies, BlockHeaders, Capabilities, DisconnectReason, EthMessage,
    EthNetworkPrimitives, EthVersion, GetBlockBodies, GetBlockHeaders, GetNodeData,
    GetPooledTransactions, GetReceipts, NetworkPrimitives, NodeData, PooledTransactions, Receipts,
    Status,
};
use reth_ethereum_forks::ForkId;
use reth_network_p2p::error::{RequestError, RequestResult};
use reth_network_peers::PeerId;
use reth_network_types::{PeerAddr, PeerKind};
use reth_tokio_util::EventStream;
use std::{
    fmt,
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::sync::{mpsc, oneshot};
use tokio_stream::{wrappers::UnboundedReceiverStream, Stream, StreamExt};

/// A boxed stream of network peer events that provides a type-erased interface.
pub struct PeerEventStream(Pin<Box<dyn Stream<Item = PeerEvent> + Send + Sync>>);

impl fmt::Debug for PeerEventStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PeerEventStream").finish_non_exhaustive()
    }
}

impl PeerEventStream {
    /// Create a new stream [`PeerEventStream`] by converting the provided stream's items into peer
    /// events [`PeerEvent`]
    pub fn new<S, T>(stream: S) -> Self
    where
        S: Stream<Item = T> + Send + Sync + 'static,
        T: Into<PeerEvent> + 'static,
    {
        let mapped_stream = stream.map(Into::into);
        Self(Box::pin(mapped_stream))
    }
}

impl Stream for PeerEventStream {
    type Item = PeerEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.0.as_mut().poll_next(cx)
    }
}

/// Represents information about an established peer session.
#[derive(Debug, Clone)]
pub struct SessionInfo {
    /// The identifier of the peer to which a session was established.
    pub peer_id: PeerId,
    /// The remote addr of the peer to which a session was established.
    pub remote_addr: SocketAddr,
    /// The client version of the peer to which a session was established.
    pub client_version: Arc<str>,
    /// Capabilities the peer announced.
    pub capabilities: Arc<Capabilities>,
    /// The status of the peer to which a session was established.
    pub status: Arc<Status>,
    /// Negotiated eth version of the session.
    pub version: EthVersion,
    /// The kind of peer this session represents
    pub peer_kind: PeerKind,
}

/// (Non-exhaustive) List of the different events emitted by the network that are of interest for
/// subscribers.
///
/// This includes any event types that may be relevant to tasks, for metrics, keep track of peers
/// etc.
#[derive(Debug, Clone)]
pub enum PeerEvent {
    /// Closed the peer session.
    SessionClosed {
        /// The identifier of the peer to which a session was closed.
        peer_id: PeerId,
        /// Why the disconnect was triggered
        reason: Option<DisconnectReason>,
    },
    /// Established a new session with the given peer.
    SessionEstablished(SessionInfo),
    /// Event emitted when a new peer is added
    PeerAdded(PeerId),
    /// Event emitted when a new peer is removed
    PeerRemoved(PeerId),
}

/// (Non-exhaustive) Network events representing peer lifecycle events and session requests.
#[derive(Debug)]
pub enum NetworkEvent<R = PeerRequest> {
    /// Basic peer lifecycle event.
    Peer(PeerEvent),
    /// Session established with requests.
    ActivePeerSession {
        /// Session information
        info: SessionInfo,
        /// A request channel to the session task.
        messages: PeerRequestSender<R>,
    },
}

impl<R> Clone for NetworkEvent<R> {
    fn clone(&self) -> Self {
        match self {
            Self::Peer(event) => Self::Peer(event.clone()),
            Self::ActivePeerSession { info, messages } => {
                Self::ActivePeerSession { info: info.clone(), messages: messages.clone() }
            }
        }
    }
}

impl<R> From<NetworkEvent<R>> for PeerEvent {
    fn from(event: NetworkEvent<R>) -> Self {
        match event {
            NetworkEvent::Peer(peer_event) => peer_event,
            NetworkEvent::ActivePeerSession { info, .. } => Self::SessionEstablished(info),
        }
    }
}

/// Provides peer event subscription for the network.
#[auto_impl::auto_impl(&, Arc)]
pub trait NetworkPeersEvents: Send + Sync {
    /// Creates a new peer event listener stream.
    fn peer_events(&self) -> PeerEventStream;
}

/// Provides event subscription for the network.
#[auto_impl::auto_impl(&, Arc)]
pub trait NetworkEventListenerProvider: NetworkPeersEvents {
    /// The primitive types to use in the `PeerRequest` used in the stream.
    type Primitives: NetworkPrimitives;

    /// Creates a new [`NetworkEvent`] listener channel.
    fn event_listener(&self) -> EventStream<NetworkEvent<PeerRequest<Self::Primitives>>>;
    /// Returns a new [`DiscoveryEvent`] stream.
    ///
    /// This stream yields [`DiscoveryEvent`]s for each peer that is discovered.
    fn discovery_listener(&self) -> UnboundedReceiverStream<DiscoveryEvent>;
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
        response: oneshot::Sender<RequestResult<PooledTransactions<N::PooledTransaction>>>,
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
        response: oneshot::Sender<RequestResult<Receipts<N::Receipt>>>,
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

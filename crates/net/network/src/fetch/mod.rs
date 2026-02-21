//! Fetch data from the network.

mod client;

pub use client::FetchClient;

use crate::{message::BlockRequest, session::BlockRangeInfo};
use alloy_primitives::B256;
use futures::StreamExt;
use reth_eth_wire::{
    Capabilities, EthNetworkPrimitives, GetBlockBodies, GetBlockHeaders, NetworkPrimitives,
};
use reth_network_api::test_utils::PeersHandle;
use reth_network_p2p::{
    error::{EthResponseValidator, PeerRequestResult, RequestError, RequestResult},
    headers::client::HeadersRequest,
    priority::Priority,
};
use reth_network_peers::PeerId;
use reth_network_types::ReputationChangeKind;
use std::{
    collections::{HashMap, VecDeque},
    ops::RangeInclusive,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc,
    },
    task::{Context, Poll},
};
use tokio::sync::{mpsc, mpsc::UnboundedSender, oneshot};
use tokio_stream::wrappers::UnboundedReceiverStream;

type InflightHeadersRequest<H> = Request<HeadersRequest, PeerRequestResult<Vec<H>>>;
type InflightBodiesRequest<B> = Request<(), PeerRequestResult<Vec<B>>>;

/// Manages data fetching operations.
///
/// This type is hooked into the staged sync pipeline and delegates download request to available
/// peers and sends the response once ready.
///
/// This type maintains a list of connected peers that are available for requests.
#[derive(Debug)]
pub struct StateFetcher<N: NetworkPrimitives = EthNetworkPrimitives> {
    /// Currently active [`GetBlockHeaders`] requests
    inflight_headers_requests: HashMap<PeerId, InflightHeadersRequest<N::BlockHeader>>,
    /// Currently active [`GetBlockBodies`] requests
    inflight_bodies_requests: HashMap<PeerId, InflightBodiesRequest<N::BlockBody>>,
    /// The list of _available_ peers for requests.
    peers: HashMap<PeerId, Peer>,
    /// The handle to the peers manager
    peers_handle: PeersHandle,
    /// Number of active peer sessions the node's currently handling.
    num_active_peers: Arc<AtomicUsize>,
    /// Requests queued for processing
    queued_requests: VecDeque<DownloadRequest<N>>,
    /// Receiver for new incoming download requests
    download_requests_rx: UnboundedReceiverStream<DownloadRequest<N>>,
    /// Sender for download requests, used to detach a [`FetchClient`]
    download_requests_tx: UnboundedSender<DownloadRequest<N>>,
}

// === impl StateSyncer ===

impl<N: NetworkPrimitives> StateFetcher<N> {
    pub(crate) fn new(peers_handle: PeersHandle, num_active_peers: Arc<AtomicUsize>) -> Self {
        let (download_requests_tx, download_requests_rx) = mpsc::unbounded_channel();
        Self {
            inflight_headers_requests: Default::default(),
            inflight_bodies_requests: Default::default(),
            peers: Default::default(),
            peers_handle,
            num_active_peers,
            queued_requests: Default::default(),
            download_requests_rx: UnboundedReceiverStream::new(download_requests_rx),
            download_requests_tx,
        }
    }

    /// Invoked when connected to a new peer.
    pub(crate) fn new_active_peer(
        &mut self,
        peer_id: PeerId,
        best_hash: B256,
        best_number: u64,
        capabilities: Arc<Capabilities>,
        timeout: Arc<AtomicU64>,
        range_info: Option<BlockRangeInfo>,
    ) {
        self.peers.insert(
            peer_id,
            Peer {
                state: PeerState::Idle,
                best_hash,
                best_number,
                capabilities,
                timeout,
                last_response_likely_bad: false,
                range_info,
            },
        );
    }

    /// Removes the peer from the peer list, after which it is no longer available for future
    /// requests.
    ///
    /// Invoked when an active session was closed.
    ///
    /// This cancels also inflight request and sends an error to the receiver.
    pub(crate) fn on_session_closed(&mut self, peer: &PeerId) {
        self.peers.remove(peer);
        if let Some(req) = self.inflight_headers_requests.remove(peer) {
            let _ = req.response.send(Err(RequestError::ConnectionDropped));
        }
        if let Some(req) = self.inflight_bodies_requests.remove(peer) {
            let _ = req.response.send(Err(RequestError::ConnectionDropped));
        }
    }

    /// Updates the block information for the peer.
    ///
    /// Returns `true` if this a newer block
    pub(crate) fn update_peer_block(&mut self, peer_id: &PeerId, hash: B256, number: u64) -> bool {
        if let Some(peer) = self.peers.get_mut(peer_id) &&
            number > peer.best_number
        {
            peer.best_hash = hash;
            peer.best_number = number;
            return true
        }
        false
    }

    /// Invoked when an active session is about to be disconnected.
    pub(crate) fn on_pending_disconnect(&mut self, peer_id: &PeerId) {
        if let Some(peer) = self.peers.get_mut(peer_id) {
            peer.state = PeerState::Closing;
        }
    }

    /// Returns the _next_ idle peer that's ready to accept a request,
    /// prioritizing those with the lowest timeout/latency and those that recently responded with
    /// adequate data. Additionally, if full blocks are required this prioritizes peers that have
    /// full history available
    fn next_best_peer(&self, requirement: BestPeerRequirements) -> Option<PeerId> {
        let mut idle = self.peers.iter().filter(|(_, peer)| peer.state.is_idle());

        let mut best_peer = idle.next()?;

        for maybe_better in idle {
            // replace best peer if our current best peer sent us a bad response last time
            if best_peer.1.last_response_likely_bad && !maybe_better.1.last_response_likely_bad {
                best_peer = maybe_better;
                continue
            }

            // replace best peer if this peer meets the requirements better
            if maybe_better.1.is_better(best_peer.1, &requirement) {
                best_peer = maybe_better;
                continue
            }

            // replace best peer if this peer has better rtt and both have same range quality
            if maybe_better.1.timeout() < best_peer.1.timeout() &&
                !maybe_better.1.last_response_likely_bad
            {
                best_peer = maybe_better;
            }
        }

        Some(*best_peer.0)
    }

    /// Returns the next action to return
    fn poll_action(&mut self) -> PollAction {
        // we only check and not pop here since we don't know yet whether a peer is available.
        if self.queued_requests.is_empty() {
            return PollAction::NoRequests
        }

        let request = self.queued_requests.pop_front().expect("not empty");
        let Some(peer_id) = self.next_best_peer(request.best_peer_requirements()) else {
            // need to put back the request
            self.queued_requests.push_front(request);
            return PollAction::NoPeersAvailable
        };

        let request = self.prepare_block_request(peer_id, request);

        PollAction::Ready(FetchAction::BlockRequest { peer_id, request })
    }

    /// Advance the state the syncer
    pub(crate) fn poll(&mut self, cx: &mut Context<'_>) -> Poll<FetchAction> {
        // drain buffered actions first
        loop {
            let no_peers_available = match self.poll_action() {
                PollAction::Ready(action) => return Poll::Ready(action),
                PollAction::NoRequests => false,
                PollAction::NoPeersAvailable => true,
            };

            loop {
                // poll incoming requests
                match self.download_requests_rx.poll_next_unpin(cx) {
                    Poll::Ready(Some(request)) => match request.get_priority() {
                        Priority::High => {
                            // find the first normal request and queue before, add this request to
                            // the back of the high-priority queue
                            let pos = self
                                .queued_requests
                                .iter()
                                .position(|req| req.is_normal_priority())
                                .unwrap_or(self.queued_requests.len());
                            self.queued_requests.insert(pos, request);
                        }
                        Priority::Normal => {
                            self.queued_requests.push_back(request);
                        }
                    },
                    Poll::Ready(None) => {
                        unreachable!("channel can't close")
                    }
                    Poll::Pending => break,
                }
            }

            if self.queued_requests.is_empty() || no_peers_available {
                return Poll::Pending
            }
        }
    }

    /// Handles a new request to a peer.
    ///
    /// Caution: this assumes the peer exists and is idle
    fn prepare_block_request(&mut self, peer_id: PeerId, req: DownloadRequest<N>) -> BlockRequest {
        // update the peer's state
        if let Some(peer) = self.peers.get_mut(&peer_id) {
            peer.state = req.peer_state();
        }

        match req {
            DownloadRequest::GetBlockHeaders { request, response, .. } => {
                let inflight = Request { request: request.clone(), response };
                self.inflight_headers_requests.insert(peer_id, inflight);
                let HeadersRequest { start, limit, direction } = request;
                BlockRequest::GetBlockHeaders(GetBlockHeaders {
                    start_block: start,
                    limit,
                    skip: 0,
                    direction,
                })
            }
            DownloadRequest::GetBlockBodies { request, response, .. } => {
                let inflight = Request { request: (), response };
                self.inflight_bodies_requests.insert(peer_id, inflight);
                BlockRequest::GetBlockBodies(GetBlockBodies(request))
            }
        }
    }

    /// Returns a new followup request for the peer.
    ///
    /// Caution: this expects that the peer is _not_ closed.
    fn followup_request(&mut self, peer_id: PeerId) -> Option<BlockResponseOutcome> {
        let req = self.queued_requests.pop_front()?;
        let req = self.prepare_block_request(peer_id, req);
        Some(BlockResponseOutcome::Request(peer_id, req))
    }

    /// Called on a `GetBlockHeaders` response from a peer.
    ///
    /// This delegates the response and returns a [`BlockResponseOutcome`] to either queue in a
    /// direct followup request or get the peer reported if the response was a
    /// [`EthResponseValidator::reputation_change_err`]
    pub(crate) fn on_block_headers_response(
        &mut self,
        peer_id: PeerId,
        res: RequestResult<Vec<N::BlockHeader>>,
    ) -> Option<BlockResponseOutcome> {
        let is_error = res.is_err();
        let maybe_reputation_change = res.reputation_change_err();

        let resp = self.inflight_headers_requests.remove(&peer_id);

        let is_likely_bad_response =
            resp.as_ref().is_some_and(|r| res.is_likely_bad_headers_response(&r.request));

        if let Some(resp) = resp {
            // delegate the response
            let _ = resp.response.send(res.map(|h| (peer_id, h).into()));
        }

        if let Some(peer) = self.peers.get_mut(&peer_id) {
            // update the peer's response state
            peer.last_response_likely_bad = is_likely_bad_response;

            // If the peer is still ready to accept new requests, we try to send a followup
            // request immediately.
            if peer.state.on_request_finished() && !is_error && !is_likely_bad_response {
                return self.followup_request(peer_id)
            }
        }

        // if the response was an `Err` worth reporting the peer for then we return a `BadResponse`
        // outcome
        maybe_reputation_change
            .map(|reputation_change| BlockResponseOutcome::BadResponse(peer_id, reputation_change))
    }

    /// Called on a `GetBlockBodies` response from a peer
    pub(crate) fn on_block_bodies_response(
        &mut self,
        peer_id: PeerId,
        res: RequestResult<Vec<N::BlockBody>>,
    ) -> Option<BlockResponseOutcome> {
        let is_likely_bad_response = res.as_ref().map_or(true, |bodies| bodies.is_empty());

        if let Some(resp) = self.inflight_bodies_requests.remove(&peer_id) {
            let _ = resp.response.send(res.map(|b| (peer_id, b).into()));
        }
        if let Some(peer) = self.peers.get_mut(&peer_id) {
            // update the peer's response state
            peer.last_response_likely_bad = is_likely_bad_response;

            if peer.state.on_request_finished() && !is_likely_bad_response {
                return self.followup_request(peer_id)
            }
        }
        None
    }

    /// Returns a new [`FetchClient`] that can send requests to this type.
    pub(crate) fn client(&self) -> FetchClient<N> {
        FetchClient {
            request_tx: self.download_requests_tx.clone(),
            peers_handle: self.peers_handle.clone(),
            num_active_peers: Arc::clone(&self.num_active_peers),
        }
    }
}

/// The outcome of [`StateFetcher::poll_action`]
enum PollAction {
    Ready(FetchAction),
    NoRequests,
    NoPeersAvailable,
}

/// Represents a connected peer
#[derive(Debug)]
struct Peer {
    /// The state this peer currently resides in.
    state: PeerState,
    /// Best known hash that the peer has
    best_hash: B256,
    /// Tracks the best number of the peer.
    best_number: u64,
    /// Capabilities announced by the peer.
    #[allow(dead_code)]
    capabilities: Arc<Capabilities>,
    /// Tracks the current timeout value we use for the peer.
    timeout: Arc<AtomicU64>,
    /// Tracks whether the peer has recently responded with a likely bad response.
    ///
    /// This is used to de-rank the peer if there are other peers available.
    /// This exists because empty responses may not be penalized (e.g. when blocks near the tip are
    /// downloaded), but we still want to avoid requesting from the same peer again if it has the
    /// lowest timeout.
    last_response_likely_bad: bool,
    /// Tracks the range info for the peer.
    range_info: Option<BlockRangeInfo>,
}

impl Peer {
    fn timeout(&self) -> u64 {
        self.timeout.load(Ordering::Relaxed)
    }

    /// Returns the earliest block number available from the peer.
    fn earliest(&self) -> u64 {
        self.range_info.as_ref().map_or(0, |info| info.earliest())
    }

    /// Returns true if the peer has the full history available.
    fn has_full_history(&self) -> bool {
        self.earliest() == 0
    }

    fn range(&self) -> Option<RangeInclusive<u64>> {
        self.range_info.as_ref().map(|info| info.range())
    }

    /// Returns true if this peer has a better range than the other peer for serving the requested
    /// range.
    ///
    /// A peer has a "better range" if:
    /// 1. It can fully cover the requested range while the other cannot
    /// 2. None can fully cover the range, but this peer has lower start value
    /// 3. If a peer doesn't announce a range we assume it has full history, but check the other's
    ///    range and treat that as better if it can cover the range
    fn has_better_range(&self, other: &Self, range: &RangeInclusive<u64>) -> bool {
        let self_range = self.range();
        let other_range = other.range();

        match (self_range, other_range) {
            (Some(self_r), Some(other_r)) => {
                // Check if each peer can fully cover the requested range
                let self_covers = self_r.contains(range.start()) && self_r.contains(range.end());
                let other_covers = other_r.contains(range.start()) && other_r.contains(range.end());

                #[allow(clippy::match_same_arms)]
                match (self_covers, other_covers) {
                    (true, false) => true,  // Only self covers the range
                    (false, true) => false, // Only other covers the range
                    (true, true) => false,  // Both cover
                    (false, false) => {
                        // neither covers - prefer if peer has lower (better) start range
                        self_r.start() < other_r.start()
                    }
                }
            }
            (Some(self_r), None) => {
                // Self has range info, other doesn't (treated as full history with unknown latest)
                // Self is better only if it covers the range
                self_r.contains(range.start()) && self_r.contains(range.end())
            }
            (None, Some(other_r)) => {
                // Self has no range info (full history), other has range info
                // Self is better only if other doesn't cover the range
                !(other_r.contains(range.start()) && other_r.contains(range.end()))
            }
            (None, None) => false, // Neither has range info - no one is better
        }
    }

    /// Returns true if this peer is better than the other peer based on the given requirements.
    fn is_better(&self, other: &Self, requirement: &BestPeerRequirements) -> bool {
        match requirement {
            BestPeerRequirements::None => false,
            BestPeerRequirements::FullBlockRange(range) => self.has_better_range(other, range),
            BestPeerRequirements::FullBlock => self.has_full_history() && !other.has_full_history(),
        }
    }
}

/// Tracks the state of an individual peer
#[derive(Debug)]
enum PeerState {
    /// Peer is currently not handling requests and is available.
    Idle,
    /// Peer is handling a `GetBlockHeaders` request.
    GetBlockHeaders,
    /// Peer is handling a `GetBlockBodies` request.
    GetBlockBodies,
    /// Peer session is about to close
    Closing,
}

// === impl PeerState ===

impl PeerState {
    /// Returns true if the peer is currently idle.
    const fn is_idle(&self) -> bool {
        matches!(self, Self::Idle)
    }

    /// Resets the state on a received response.
    ///
    /// If the state was already marked as `Closing` do nothing.
    ///
    /// Returns `true` if the peer is ready for another request.
    const fn on_request_finished(&mut self) -> bool {
        if !matches!(self, Self::Closing) {
            *self = Self::Idle;
            return true
        }
        false
    }
}

/// A request that waits for a response from the network, so it can send it back through the
/// response channel.
#[derive(Debug)]
struct Request<Req, Resp> {
    /// The issued request object
    // TODO: this can be attached to the response in error case
    request: Req,
    response: oneshot::Sender<Resp>,
}

/// Requests that can be sent to the Syncer from a [`FetchClient`]
#[derive(Debug)]
pub(crate) enum DownloadRequest<N: NetworkPrimitives> {
    /// Download the requested headers and send response through channel
    GetBlockHeaders {
        request: HeadersRequest,
        response: oneshot::Sender<PeerRequestResult<Vec<N::BlockHeader>>>,
        priority: Priority,
    },
    /// Download the requested headers and send response through channel
    GetBlockBodies {
        request: Vec<B256>,
        response: oneshot::Sender<PeerRequestResult<Vec<N::BlockBody>>>,
        priority: Priority,
        range_hint: Option<RangeInclusive<u64>>,
    },
}

// === impl DownloadRequest ===

impl<N: NetworkPrimitives> DownloadRequest<N> {
    /// Returns the corresponding state for a peer that handles the request.
    const fn peer_state(&self) -> PeerState {
        match self {
            Self::GetBlockHeaders { .. } => PeerState::GetBlockHeaders,
            Self::GetBlockBodies { .. } => PeerState::GetBlockBodies,
        }
    }

    /// Returns the requested priority of this request
    const fn get_priority(&self) -> &Priority {
        match self {
            Self::GetBlockHeaders { priority, .. } | Self::GetBlockBodies { priority, .. } => {
                priority
            }
        }
    }

    /// Returns `true` if this request is normal priority.
    const fn is_normal_priority(&self) -> bool {
        self.get_priority().is_normal()
    }

    /// Returns the best peer requirements for this request.
    fn best_peer_requirements(&self) -> BestPeerRequirements {
        match self {
            Self::GetBlockHeaders { .. } => BestPeerRequirements::None,
            Self::GetBlockBodies { range_hint, .. } => {
                if let Some(range) = range_hint {
                    BestPeerRequirements::FullBlockRange(range.clone())
                } else {
                    BestPeerRequirements::FullBlock
                }
            }
        }
    }
}

/// An action the syncer can emit.
pub(crate) enum FetchAction {
    /// Dispatch an eth request to the given peer.
    BlockRequest {
        /// The targeted recipient for the request
        peer_id: PeerId,
        /// The request to send
        request: BlockRequest,
    },
}

/// Outcome of a processed response.
///
/// Returned after processing a response.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum BlockResponseOutcome {
    /// Continue with another request to the peer.
    Request(PeerId, BlockRequest),
    /// How to handle a bad response and the reputation change to apply, if any.
    BadResponse(PeerId, ReputationChangeKind),
}

/// Additional requirements for how to rank peers during selection.
enum BestPeerRequirements {
    /// No additional requirements
    None,
    /// Peer must have this block range available.
    FullBlockRange(RangeInclusive<u64>),
    /// Peer must have full range.
    FullBlock,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{peers::PeersManager, PeersConfig};
    use alloy_consensus::Header;
    use alloy_primitives::B512;
    use std::future::poll_fn;

    #[tokio::test(flavor = "multi_thread")]
    async fn test_poll_fetcher() {
        let manager = PeersManager::new(PeersConfig::default());
        let mut fetcher =
            StateFetcher::<EthNetworkPrimitives>::new(manager.handle(), Default::default());

        poll_fn(move |cx| {
            assert!(fetcher.poll(cx).is_pending());
            let (tx, _rx) = oneshot::channel();
            fetcher.queued_requests.push_back(DownloadRequest::GetBlockBodies {
                request: vec![],
                response: tx,
                priority: Priority::default(),
                range_hint: None,
            });
            assert!(fetcher.poll(cx).is_pending());

            Poll::Ready(())
        })
        .await;
    }

    #[tokio::test]
    async fn test_peer_rotation() {
        let manager = PeersManager::new(PeersConfig::default());
        let mut fetcher =
            StateFetcher::<EthNetworkPrimitives>::new(manager.handle(), Default::default());
        // Add a few random peers
        let peer1 = B512::random();
        let peer2 = B512::random();
        let capabilities = Arc::new(Capabilities::from(vec![]));
        fetcher.new_active_peer(
            peer1,
            B256::random(),
            1,
            Arc::clone(&capabilities),
            Arc::new(AtomicU64::new(1)),
            None,
        );
        fetcher.new_active_peer(
            peer2,
            B256::random(),
            2,
            Arc::clone(&capabilities),
            Arc::new(AtomicU64::new(1)),
            None,
        );

        let first_peer = fetcher.next_best_peer(BestPeerRequirements::None).unwrap();
        assert!(first_peer == peer1 || first_peer == peer2);
        // Pending disconnect for first_peer
        fetcher.on_pending_disconnect(&first_peer);
        // first_peer now isn't idle, so we should get other peer
        let second_peer = fetcher.next_best_peer(BestPeerRequirements::None).unwrap();
        assert!(first_peer == peer1 || first_peer == peer2);
        assert_ne!(first_peer, second_peer);
        // without idle peers, returns None
        fetcher.on_pending_disconnect(&second_peer);
        assert_eq!(fetcher.next_best_peer(BestPeerRequirements::None), None);
    }

    #[tokio::test]
    async fn test_peer_prioritization() {
        let manager = PeersManager::new(PeersConfig::default());
        let mut fetcher =
            StateFetcher::<EthNetworkPrimitives>::new(manager.handle(), Default::default());
        // Add a few random peers
        let peer1 = B512::random();
        let peer2 = B512::random();
        let peer3 = B512::random();

        let peer2_timeout = Arc::new(AtomicU64::new(300));

        let capabilities = Arc::new(Capabilities::from(vec![]));
        fetcher.new_active_peer(
            peer1,
            B256::random(),
            1,
            Arc::clone(&capabilities),
            Arc::new(AtomicU64::new(30)),
            None,
        );
        fetcher.new_active_peer(
            peer2,
            B256::random(),
            2,
            Arc::clone(&capabilities),
            Arc::clone(&peer2_timeout),
            None,
        );
        fetcher.new_active_peer(
            peer3,
            B256::random(),
            3,
            Arc::clone(&capabilities),
            Arc::new(AtomicU64::new(50)),
            None,
        );

        // Must always get peer1 (lowest timeout)
        assert_eq!(fetcher.next_best_peer(BestPeerRequirements::None), Some(peer1));
        assert_eq!(fetcher.next_best_peer(BestPeerRequirements::None), Some(peer1));
        // peer2's timeout changes below peer1's
        peer2_timeout.store(10, Ordering::Relaxed);
        // Then we get peer 2 always (now lowest)
        assert_eq!(fetcher.next_best_peer(BestPeerRequirements::None), Some(peer2));
        assert_eq!(fetcher.next_best_peer(BestPeerRequirements::None), Some(peer2));
    }

    #[tokio::test]
    async fn test_on_block_headers_response() {
        let manager = PeersManager::new(PeersConfig::default());
        let mut fetcher =
            StateFetcher::<EthNetworkPrimitives>::new(manager.handle(), Default::default());
        let peer_id = B512::random();

        assert_eq!(fetcher.on_block_headers_response(peer_id, Ok(vec![Header::default()])), None);

        assert_eq!(
            fetcher.on_block_headers_response(peer_id, Err(RequestError::Timeout)),
            Some(BlockResponseOutcome::BadResponse(peer_id, ReputationChangeKind::Timeout))
        );
        assert_eq!(
            fetcher.on_block_headers_response(peer_id, Err(RequestError::BadResponse)),
            None
        );
        assert_eq!(
            fetcher.on_block_headers_response(peer_id, Err(RequestError::ChannelClosed)),
            None
        );
        assert_eq!(
            fetcher.on_block_headers_response(peer_id, Err(RequestError::ConnectionDropped)),
            None
        );
        assert_eq!(
            fetcher.on_block_headers_response(peer_id, Err(RequestError::UnsupportedCapability)),
            None
        );
    }

    #[tokio::test]
    async fn test_header_response_outcome() {
        let manager = PeersManager::new(PeersConfig::default());
        let mut fetcher =
            StateFetcher::<EthNetworkPrimitives>::new(manager.handle(), Default::default());
        let peer_id = B512::random();

        let request_pair = || {
            let (tx, _rx) = oneshot::channel();
            let req = Request {
                request: HeadersRequest {
                    start: 0u64.into(),
                    limit: 1,
                    direction: Default::default(),
                },
                response: tx,
            };
            let header = Header { number: 0, ..Default::default() };
            (req, header)
        };

        fetcher.new_active_peer(
            peer_id,
            Default::default(),
            Default::default(),
            Arc::new(Capabilities::from(vec![])),
            Default::default(),
            None,
        );

        let (req, header) = request_pair();
        fetcher.inflight_headers_requests.insert(peer_id, req);

        let outcome = fetcher.on_block_headers_response(peer_id, Ok(vec![header]));
        assert!(outcome.is_none());
        assert!(fetcher.peers[&peer_id].state.is_idle());

        let outcome =
            fetcher.on_block_headers_response(peer_id, Err(RequestError::Timeout)).unwrap();

        assert!(EthResponseValidator::reputation_change_err(&Err::<Vec<Header>, _>(
            RequestError::Timeout
        ))
        .is_some());

        match outcome {
            BlockResponseOutcome::BadResponse(peer, _) => {
                assert_eq!(peer, peer_id)
            }
            BlockResponseOutcome::Request(_, _) => {
                unreachable!()
            }
        };

        assert!(fetcher.peers[&peer_id].state.is_idle());
    }

    #[test]
    fn test_peer_is_better_none_requirement() {
        let peer1 = Peer {
            state: PeerState::Idle,
            best_hash: B256::random(),
            best_number: 100,
            capabilities: Arc::new(Capabilities::new(vec![])),
            timeout: Arc::new(AtomicU64::new(10)),
            last_response_likely_bad: false,
            range_info: Some(BlockRangeInfo::new(0, 100, B256::random())),
        };

        let peer2 = Peer {
            state: PeerState::Idle,
            best_hash: B256::random(),
            best_number: 50,
            capabilities: Arc::new(Capabilities::new(vec![])),
            timeout: Arc::new(AtomicU64::new(20)),
            last_response_likely_bad: false,
            range_info: None,
        };

        // With None requirement, is_better should always return false
        assert!(!peer1.is_better(&peer2, &BestPeerRequirements::None));
        assert!(!peer2.is_better(&peer1, &BestPeerRequirements::None));
    }

    #[test]
    fn test_peer_is_better_full_block_requirement() {
        // Peer with full history (earliest = 0)
        let peer_full = Peer {
            state: PeerState::Idle,
            best_hash: B256::random(),
            best_number: 100,
            capabilities: Arc::new(Capabilities::new(vec![])),
            timeout: Arc::new(AtomicU64::new(10)),
            last_response_likely_bad: false,
            range_info: Some(BlockRangeInfo::new(0, 100, B256::random())),
        };

        // Peer without full history (earliest = 50)
        let peer_partial = Peer {
            state: PeerState::Idle,
            best_hash: B256::random(),
            best_number: 100,
            capabilities: Arc::new(Capabilities::new(vec![])),
            timeout: Arc::new(AtomicU64::new(10)),
            last_response_likely_bad: false,
            range_info: Some(BlockRangeInfo::new(50, 100, B256::random())),
        };

        // Peer without range info (treated as full history)
        let peer_no_range = Peer {
            state: PeerState::Idle,
            best_hash: B256::random(),
            best_number: 100,
            capabilities: Arc::new(Capabilities::new(vec![])),
            timeout: Arc::new(AtomicU64::new(10)),
            last_response_likely_bad: false,
            range_info: None,
        };

        // Peer with full history is better than peer without
        assert!(peer_full.is_better(&peer_partial, &BestPeerRequirements::FullBlock));
        assert!(!peer_partial.is_better(&peer_full, &BestPeerRequirements::FullBlock));

        // Peer without range info (full history) is better than partial
        assert!(peer_no_range.is_better(&peer_partial, &BestPeerRequirements::FullBlock));
        assert!(!peer_partial.is_better(&peer_no_range, &BestPeerRequirements::FullBlock));

        // Both have full history - no improvement
        assert!(!peer_full.is_better(&peer_no_range, &BestPeerRequirements::FullBlock));
        assert!(!peer_no_range.is_better(&peer_full, &BestPeerRequirements::FullBlock));
    }

    #[test]
    fn test_peer_is_better_full_block_range_requirement() {
        let range = RangeInclusive::new(40, 60);

        // Peer that covers the requested range
        let peer_covers = Peer {
            state: PeerState::Idle,
            best_hash: B256::random(),
            best_number: 100,
            capabilities: Arc::new(Capabilities::new(vec![])),
            timeout: Arc::new(AtomicU64::new(10)),
            last_response_likely_bad: false,
            range_info: Some(BlockRangeInfo::new(0, 100, B256::random())),
        };

        // Peer that doesn't cover the range (earliest too high)
        let peer_no_cover = Peer {
            state: PeerState::Idle,
            best_hash: B256::random(),
            best_number: 100,
            capabilities: Arc::new(Capabilities::new(vec![])),
            timeout: Arc::new(AtomicU64::new(10)),
            last_response_likely_bad: false,
            range_info: Some(BlockRangeInfo::new(70, 100, B256::random())),
        };

        // Peer that covers the requested range is better than one that doesn't
        assert!(peer_covers
            .is_better(&peer_no_cover, &BestPeerRequirements::FullBlockRange(range.clone())));
        assert!(
            !peer_no_cover.is_better(&peer_covers, &BestPeerRequirements::FullBlockRange(range))
        );
    }

    #[test]
    fn test_peer_is_better_both_cover_range() {
        let range = RangeInclusive::new(30, 50);

        // Peer with full history that covers the range
        let peer_full = Peer {
            state: PeerState::Idle,
            best_hash: B256::random(),
            best_number: 100,
            capabilities: Arc::new(Capabilities::new(vec![])),
            timeout: Arc::new(AtomicU64::new(10)),
            last_response_likely_bad: false,
            range_info: Some(BlockRangeInfo::new(0, 50, B256::random())),
        };

        // Peer without full history that also covers the range
        let peer_partial = Peer {
            state: PeerState::Idle,
            best_hash: B256::random(),
            best_number: 100,
            capabilities: Arc::new(Capabilities::new(vec![])),
            timeout: Arc::new(AtomicU64::new(10)),
            last_response_likely_bad: false,
            range_info: Some(BlockRangeInfo::new(30, 50, B256::random())),
        };

        // When both cover the range, prefer none
        assert!(!peer_full
            .is_better(&peer_partial, &BestPeerRequirements::FullBlockRange(range.clone())));
        assert!(!peer_partial.is_better(&peer_full, &BestPeerRequirements::FullBlockRange(range)));
    }

    #[test]
    fn test_peer_is_better_lower_start() {
        let range = RangeInclusive::new(30, 60);

        // Peer with full history that covers the range
        let peer_full = Peer {
            state: PeerState::Idle,
            best_hash: B256::random(),
            best_number: 100,
            capabilities: Arc::new(Capabilities::new(vec![])),
            timeout: Arc::new(AtomicU64::new(10)),
            last_response_likely_bad: false,
            range_info: Some(BlockRangeInfo::new(0, 50, B256::random())),
        };

        // Peer without full history that also covers the range
        let peer_partial = Peer {
            state: PeerState::Idle,
            best_hash: B256::random(),
            best_number: 100,
            capabilities: Arc::new(Capabilities::new(vec![])),
            timeout: Arc::new(AtomicU64::new(10)),
            last_response_likely_bad: false,
            range_info: Some(BlockRangeInfo::new(30, 50, B256::random())),
        };

        // When both cover the range, prefer lower start value
        assert!(peer_full
            .is_better(&peer_partial, &BestPeerRequirements::FullBlockRange(range.clone())));
        assert!(!peer_partial.is_better(&peer_full, &BestPeerRequirements::FullBlockRange(range)));
    }

    #[test]
    fn test_peer_is_better_neither_covers_range() {
        let range = RangeInclusive::new(40, 60);

        // Peer with full history that doesn't cover the range (latest too low)
        let peer_full = Peer {
            state: PeerState::Idle,
            best_hash: B256::random(),
            best_number: 30,
            capabilities: Arc::new(Capabilities::new(vec![])),
            timeout: Arc::new(AtomicU64::new(10)),
            last_response_likely_bad: false,
            range_info: Some(BlockRangeInfo::new(0, 30, B256::random())),
        };

        // Peer without full history that also doesn't cover the range
        let peer_partial = Peer {
            state: PeerState::Idle,
            best_hash: B256::random(),
            best_number: 30,
            capabilities: Arc::new(Capabilities::new(vec![])),
            timeout: Arc::new(AtomicU64::new(10)),
            last_response_likely_bad: false,
            range_info: Some(BlockRangeInfo::new(10, 30, B256::random())),
        };

        // When neither covers the range, prefer full history
        assert!(peer_full
            .is_better(&peer_partial, &BestPeerRequirements::FullBlockRange(range.clone())));
        assert!(!peer_partial.is_better(&peer_full, &BestPeerRequirements::FullBlockRange(range)));
    }

    #[test]
    fn test_peer_is_better_no_range_info() {
        let range = RangeInclusive::new(40, 60);

        // Peer with range info
        let peer_with_range = Peer {
            state: PeerState::Idle,
            best_hash: B256::random(),
            best_number: 100,
            capabilities: Arc::new(Capabilities::new(vec![])),
            timeout: Arc::new(AtomicU64::new(10)),
            last_response_likely_bad: false,
            range_info: Some(BlockRangeInfo::new(30, 100, B256::random())),
        };

        // Peer without range info
        let peer_no_range = Peer {
            state: PeerState::Idle,
            best_hash: B256::random(),
            best_number: 100,
            capabilities: Arc::new(Capabilities::new(vec![])),
            timeout: Arc::new(AtomicU64::new(10)),
            last_response_likely_bad: false,
            range_info: None,
        };

        // Peer without range info is not better (we prefer peers with known ranges)
        assert!(!peer_no_range
            .is_better(&peer_with_range, &BestPeerRequirements::FullBlockRange(range.clone())));

        // Peer with range info is better than peer without
        assert!(
            peer_with_range.is_better(&peer_no_range, &BestPeerRequirements::FullBlockRange(range))
        );
    }

    #[test]
    fn test_peer_is_better_one_peer_no_range_covers() {
        let range = RangeInclusive::new(40, 60);

        // Peer with range info that covers the requested range
        let peer_with_range_covers = Peer {
            state: PeerState::Idle,
            best_hash: B256::random(),
            best_number: 100,
            capabilities: Arc::new(Capabilities::new(vec![])),
            timeout: Arc::new(AtomicU64::new(10)),
            last_response_likely_bad: false,
            range_info: Some(BlockRangeInfo::new(30, 100, B256::random())),
        };

        // Peer without range info (treated as full history with unknown latest)
        let peer_no_range = Peer {
            state: PeerState::Idle,
            best_hash: B256::random(),
            best_number: 100,
            capabilities: Arc::new(Capabilities::new(vec![])),
            timeout: Arc::new(AtomicU64::new(10)),
            last_response_likely_bad: false,
            range_info: None,
        };

        // Peer with range that covers is better than peer without range info
        assert!(peer_with_range_covers
            .is_better(&peer_no_range, &BestPeerRequirements::FullBlockRange(range.clone())));

        // Peer without range info is not better when other covers
        assert!(!peer_no_range
            .is_better(&peer_with_range_covers, &BestPeerRequirements::FullBlockRange(range)));
    }

    #[test]
    fn test_peer_is_better_one_peer_no_range_doesnt_cover() {
        let range = RangeInclusive::new(40, 60);

        // Peer with range info that does NOT cover the requested range (too high)
        let peer_with_range_no_cover = Peer {
            state: PeerState::Idle,
            best_hash: B256::random(),
            best_number: 100,
            capabilities: Arc::new(Capabilities::new(vec![])),
            timeout: Arc::new(AtomicU64::new(10)),
            last_response_likely_bad: false,
            range_info: Some(BlockRangeInfo::new(70, 100, B256::random())),
        };

        // Peer without range info (treated as full history)
        let peer_no_range = Peer {
            state: PeerState::Idle,
            best_hash: B256::random(),
            best_number: 100,
            capabilities: Arc::new(Capabilities::new(vec![])),
            timeout: Arc::new(AtomicU64::new(10)),
            last_response_likely_bad: false,
            range_info: None,
        };

        // Peer with range that doesn't cover is not better
        assert!(!peer_with_range_no_cover
            .is_better(&peer_no_range, &BestPeerRequirements::FullBlockRange(range.clone())));

        // Peer without range info (full history) is better when other doesn't cover
        assert!(peer_no_range
            .is_better(&peer_with_range_no_cover, &BestPeerRequirements::FullBlockRange(range)));
    }

    #[test]
    fn test_peer_is_better_edge_cases() {
        // Test exact range boundaries
        let range = RangeInclusive::new(50, 100);

        // Peer that exactly covers the range
        let peer_exact = Peer {
            state: PeerState::Idle,
            best_hash: B256::random(),
            best_number: 100,
            capabilities: Arc::new(Capabilities::new(vec![])),
            timeout: Arc::new(AtomicU64::new(10)),
            last_response_likely_bad: false,
            range_info: Some(BlockRangeInfo::new(50, 100, B256::random())),
        };

        // Peer that's one block short at the start
        let peer_short_start = Peer {
            state: PeerState::Idle,
            best_hash: B256::random(),
            best_number: 100,
            capabilities: Arc::new(Capabilities::new(vec![])),
            timeout: Arc::new(AtomicU64::new(10)),
            last_response_likely_bad: false,
            range_info: Some(BlockRangeInfo::new(51, 100, B256::random())),
        };

        // Peer that's one block short at the end
        let peer_short_end = Peer {
            state: PeerState::Idle,
            best_hash: B256::random(),
            best_number: 100,
            capabilities: Arc::new(Capabilities::new(vec![])),
            timeout: Arc::new(AtomicU64::new(10)),
            last_response_likely_bad: false,
            range_info: Some(BlockRangeInfo::new(50, 99, B256::random())),
        };

        // Exact coverage is better than short coverage
        assert!(peer_exact
            .is_better(&peer_short_start, &BestPeerRequirements::FullBlockRange(range.clone())));
        assert!(peer_exact
            .is_better(&peer_short_end, &BestPeerRequirements::FullBlockRange(range.clone())));

        // Short coverage is not better than exact coverage
        assert!(!peer_short_start
            .is_better(&peer_exact, &BestPeerRequirements::FullBlockRange(range.clone())));
        assert!(
            !peer_short_end.is_better(&peer_exact, &BestPeerRequirements::FullBlockRange(range))
        );
    }
}

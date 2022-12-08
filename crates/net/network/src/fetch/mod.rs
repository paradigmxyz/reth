//! Fetch data from the network.

use crate::message::BlockRequest;
use futures::StreamExt;
use reth_eth_wire::{BlockBody, GetBlockBodies, GetBlockHeaders};
use reth_interfaces::p2p::{
    error::{RequestError, RequestResult},
    headers::client::HeadersRequest,
};
use reth_primitives::{Header, PeerId, H256, U256};
use std::{
    collections::{HashMap, VecDeque},
    task::{Context, Poll},
};
use tokio::sync::{mpsc, mpsc::UnboundedSender, oneshot};
use tokio_stream::wrappers::UnboundedReceiverStream;

mod client;
use crate::peers::ReputationChangeKind;
pub use client::FetchClient;

/// Manages data fetching operations.
///
/// This type is hooked into the staged sync pipeline and delegates download request to available
/// peers and sends the response once ready.
pub struct StateFetcher {
    /// Currently active [`GetBlockHeaders`] requests
    inflight_headers_requests: HashMap<PeerId, Request<HeadersRequest, RequestResult<Vec<Header>>>>,
    /// Currently active [`GetBlockBodies`] requests
    inflight_bodies_requests: HashMap<PeerId, Request<Vec<H256>, RequestResult<Vec<BlockBody>>>>,
    /// The list of available peers for requests.
    peers: HashMap<PeerId, Peer>,
    /// Requests queued for processing
    queued_requests: VecDeque<DownloadRequest>,
    /// Receiver for new incoming download requests
    download_requests_rx: UnboundedReceiverStream<DownloadRequest>,
    /// Sender for download requests, used to detach a [`FetchClient`]
    download_requests_tx: UnboundedSender<DownloadRequest>,
    /// Receiver for new incoming [`StatusUpdate`] requests.
    status_rx: UnboundedReceiverStream<StatusUpdate>,
    /// Sender for updating the status, used to detach a [`FetchClient`]
    status_tx: UnboundedSender<StatusUpdate>,
}

// === impl StateSyncer ===

impl StateFetcher {
    /// Invoked when connected to a new peer.
    pub(crate) fn new_active_peer(&mut self, peer_id: PeerId, best_hash: H256, best_number: u64) {
        self.peers.insert(peer_id, Peer { state: PeerState::Idle, best_hash, best_number });
    }

    /// Invoked when an active session was closed.
    ///
    /// This cancels als inflight request and sends an error to the receiver.
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
    pub(crate) fn update_peer_block(&mut self, peer_id: &PeerId, hash: H256, number: u64) -> bool {
        if let Some(peer) = self.peers.get_mut(peer_id) {
            if number > peer.best_number {
                peer.best_hash = hash;
                peer.best_number = number;
                return true
            }
        }
        false
    }

    /// Invoked when an active session is about to be disconnected.
    pub(crate) fn on_pending_disconnect(&mut self, peer_id: &PeerId) {
        if let Some(peer) = self.peers.get_mut(peer_id) {
            peer.state = PeerState::Closing;
        }
    }

    /// Returns the next idle peer that's ready to accept a request
    fn next_peer(&mut self) -> Option<(&PeerId, &mut Peer)> {
        self.peers.iter_mut().find(|(_, peer)| peer.state.is_idle())
    }

    /// Returns the next action to return
    fn poll_action(&mut self) -> PollAction {
        // we only check and not pop here since we don't know yet whether a peer is available.
        if self.queued_requests.is_empty() {
            return PollAction::NoRequests
        }

        let peer_id = if let Some(peer) = self.next_peer() {
            *peer.0
        } else {
            return PollAction::NoPeersAvailable
        };

        let request = self.queued_requests.pop_front().expect("not empty; qed");
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

            if let Poll::Ready(Some(status)) = self.status_rx.poll_next_unpin(cx) {
                return Poll::Ready(FetchAction::StatusUpdate(status))
            }

            loop {
                // poll incoming requests
                match self.download_requests_rx.poll_next_unpin(cx) {
                    Poll::Ready(Some(request)) => {
                        self.queued_requests.push_back(request);
                    }
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
    fn prepare_block_request(&mut self, peer_id: PeerId, req: DownloadRequest) -> BlockRequest {
        // update the peer's state
        if let Some(peer) = self.peers.get_mut(&peer_id) {
            peer.state = req.peer_state();
        }

        match req {
            DownloadRequest::GetBlockHeaders { request, response } => {
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
            DownloadRequest::GetBlockBodies { request, response } => {
                let inflight = Request { request: request.clone(), response };
                self.inflight_bodies_requests.insert(peer_id, inflight);
                BlockRequest::GetBlockBodies(GetBlockBodies(request))
            }
        }
    }

    /// Returns a new followup request for the peer.
    ///
    /// Caution: this expects that the peer is _not_ closed
    fn followup_request(&mut self, peer_id: PeerId) -> Option<BlockResponseOutcome> {
        let req = self.queued_requests.pop_front()?;
        let req = self.prepare_block_request(peer_id, req);
        Some(BlockResponseOutcome::Request(peer_id, req))
    }

    /// Called on a `GetBlockHeaders` response from a peer
    pub(crate) fn on_block_headers_response(
        &mut self,
        peer_id: PeerId,
        res: RequestResult<Vec<Header>>,
    ) -> Option<BlockResponseOutcome> {
        let is_error = res.is_err();
        if let Some(resp) = self.inflight_headers_requests.remove(&peer_id) {
            let _ = resp.response.send(res);
        }

        if is_error {
            // if the response was erroneous we want to report the peer.
            return Some(BlockResponseOutcome::BadResponse(
                peer_id,
                ReputationChangeKind::BadMessage,
            ))
        }

        if let Some(peer) = self.peers.get_mut(&peer_id) {
            // If the peer is still ready to be accept new requests, we try to send a followup
            // request immediately.
            if peer.state.on_request_finished() {
                return self.followup_request(peer_id)
            }
        }
        None
    }

    /// Called on a `GetBlockBodies` response from a peer
    pub(crate) fn on_block_bodies_response(
        &mut self,
        peer_id: PeerId,
        res: RequestResult<Vec<BlockBody>>,
    ) -> Option<BlockResponseOutcome> {
        if let Some(resp) = self.inflight_bodies_requests.remove(&peer_id) {
            let _ = resp.response.send(res);
        }
        if let Some(peer) = self.peers.get_mut(&peer_id) {
            if peer.state.on_request_finished() {
                return self.followup_request(peer_id)
            }
        }
        None
    }

    /// Returns a new [`FetchClient`] that can send requests to this type.
    pub(crate) fn client(&self) -> FetchClient {
        FetchClient {
            request_tx: self.download_requests_tx.clone(),
            status_tx: self.status_tx.clone(),
        }
    }
}

impl Default for StateFetcher {
    fn default() -> Self {
        let (download_requests_tx, download_requests_rx) = mpsc::unbounded_channel();
        let (status_tx, status_rx) = mpsc::unbounded_channel();
        Self {
            inflight_headers_requests: Default::default(),
            inflight_bodies_requests: Default::default(),
            peers: Default::default(),
            queued_requests: Default::default(),
            download_requests_rx: UnboundedReceiverStream::new(download_requests_rx),
            download_requests_tx,
            status_rx: UnboundedReceiverStream::new(status_rx),
            status_tx,
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
struct Peer {
    /// The state this peer currently resides in.
    state: PeerState,
    /// Best known hash that the peer has
    best_hash: H256,
    /// Tracks the best number of the peer.
    best_number: u64,
}

/// Tracks the state of an individual peer
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
    fn is_idle(&self) -> bool {
        matches!(self, PeerState::Idle)
    }

    /// Resets the state on a received response.
    ///
    /// If the state was already marked as `Closing` do nothing.
    ///
    /// Returns `true` if the peer is ready for another request.
    fn on_request_finished(&mut self) -> bool {
        if !matches!(self, PeerState::Closing) {
            *self = PeerState::Idle;
            return true
        }
        false
    }
}

/// A request that waits for a response from the network, so it can send it back through the
/// response channel.
struct Request<Req, Resp> {
    /// The issued request object
    /// TODO: this can be attached to the response in error case
    #[allow(unused)]
    request: Req,
    response: oneshot::Sender<Resp>,
}

/// A message to update the status.
#[derive(Debug, Clone)]
pub(crate) struct StatusUpdate {
    pub(crate) height: u64,
    pub(crate) hash: H256,
    pub(crate) total_difficulty: U256,
}

/// Requests that can be sent to the Syncer from a [`FetchClient`]
pub(crate) enum DownloadRequest {
    /// Download the requested headers and send response through channel
    GetBlockHeaders {
        request: HeadersRequest,
        response: oneshot::Sender<RequestResult<Vec<Header>>>,
    },
    /// Download the requested headers and send response through channel
    GetBlockBodies { request: Vec<H256>, response: oneshot::Sender<RequestResult<Vec<BlockBody>>> },
}

// === impl DownloadRequest ===

impl DownloadRequest {
    /// Returns the corresponding state for a peer that handles the request.
    fn peer_state(&self) -> PeerState {
        match self {
            DownloadRequest::GetBlockHeaders { .. } => PeerState::GetBlockHeaders,
            DownloadRequest::GetBlockBodies { .. } => PeerState::GetBlockBodies,
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
    /// Propagate a received status update for the node
    StatusUpdate(StatusUpdate),
}

/// Outcome of a processed response.
///
/// Returned after processing a response.
#[derive(Debug)]
pub(crate) enum BlockResponseOutcome {
    /// Continue with another request to the peer.
    Request(PeerId, BlockRequest),
    /// How to handle a bad response and the reputation change to apply.
    BadResponse(PeerId, ReputationChangeKind),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::poll_fn;

    #[tokio::test(flavor = "multi_thread")]
    async fn test_poll_fetcher() {
        let mut fetcher = StateFetcher::default();

        poll_fn(move |cx| {
            assert!(fetcher.poll(cx).is_pending());
            let (tx, _rx) = oneshot::channel();
            fetcher
                .queued_requests
                .push_back(DownloadRequest::GetBlockBodies { request: vec![], response: tx });
            assert!(fetcher.poll(cx).is_pending());

            Poll::Ready(())
        })
        .await;
    }
}

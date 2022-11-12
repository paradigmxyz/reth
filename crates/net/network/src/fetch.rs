//! Fetch data from the network.

use crate::{message::BlockRequest, NodeId};
use futures::StreamExt;
use reth_eth_wire::{BlockBody, EthMessage};
use reth_interfaces::p2p::{error::RequestResult, headers::client::HeadersRequest};
use reth_primitives::{Header, H256, U256};
use std::{
    collections::{HashMap, VecDeque},
    task::{Context, Poll},
    time::Instant,
};
use tokio::sync::{mpsc, mpsc::UnboundedSender, oneshot};
use tokio_stream::wrappers::UnboundedReceiverStream;

/// Manages data fetching operations.
///
/// This type is hooked into the staged sync pipeline and delegates download request to available
/// peers and sends the response once ready.
pub struct StateFetcher {
    /// Currently active [`GetBlockHeaders`] requests
    inflight_headers_requests: HashMap<NodeId, Request<HeadersRequest, RequestResult<Vec<Header>>>>,
    /// The list of available peers for requests.
    peers: HashMap<NodeId, Peer>,
    /// Requests queued for processing
    queued_requests: VecDeque<DownloadRequest>,
    /// Receiver for new incoming download requests
    download_requests_rx: UnboundedReceiverStream<DownloadRequest>,
    /// Sender for download requests, used to detach a [`HeadersDownloader`]
    download_requests_tx: UnboundedSender<DownloadRequest>,
}

// === impl StateSyncer ===

impl StateFetcher {
    /// Invoked when connected to a new peer.
    pub(crate) fn new_connected_peer(&mut self, _node_id: NodeId, _best_hash: H256) {}

    /// Invoked when an active session was closed.
    pub(crate) fn on_session_closed(&mut self, _peer: &NodeId) {}

    /// Invoked when an active session is about to be disconnected.
    pub(crate) fn on_pending_disconnect(&mut self, _peer: &NodeId) {}

    /// Returns the next action to return
    fn poll_action(&mut self) -> Option<FetchAction> {
        // TODO find matching peers

        // if let Some(request) = self.queued_requests.pop_front() {
        //     if let Some(action) = self.on_download_request(request) {
        //         return Poll::Ready(action)
        //     }
        // }
        None
    }

    fn on_download_request(&mut self, request: DownloadRequest) -> Option<FetchAction> {
        match request {
            DownloadRequest::GetBlockHeaders { request: _, response: _ } => {}
            DownloadRequest::GetBlockBodies { .. } => {}
        }
        None
    }

    /// Advance the state the syncer
    pub(crate) fn poll(&mut self, cx: &mut Context<'_>) -> Poll<FetchAction> {
        // drain buffered actions first
        if let Some(action) = self.poll_action() {
            return Poll::Ready(action)
        }

        loop {
            // poll incoming requests
            match self.download_requests_rx.poll_next_unpin(cx) {
                Poll::Ready(Some(request)) => {
                    if let Some(action) = self.on_download_request(request) {
                        return Poll::Ready(action)
                    }
                }
                Poll::Ready(None) => {
                    unreachable!("channel can't close")
                }
                Poll::Pending => break,
            }
        }

        if self.queued_requests.is_empty() {
            return Poll::Pending
        }

        Poll::Pending
    }

    /// Called on a `GetBlockHeaders` response from a peer
    pub(crate) fn on_block_headers_response(
        &mut self,
        _peer: NodeId,
        _res: RequestResult<Vec<Header>>,
    ) -> Option<BlockResponseOutcome> {
        None
    }

    /// Called on a `GetBlockBodies` response from a peer
    pub(crate) fn on_block_bodies_response(
        &mut self,
        _peer: NodeId,
        _res: RequestResult<Vec<BlockBody>>,
    ) -> Option<BlockResponseOutcome> {
        None
    }

    /// Returns a new [`HeadersDownloader`] that can send requests to this type
    pub(crate) fn headers_downloader(&self) -> HeadersDownloader {
        HeadersDownloader { request_tx: self.download_requests_tx.clone() }
    }
}

impl Default for StateFetcher {
    fn default() -> Self {
        let (download_requests_tx, download_requests_rx) = mpsc::unbounded_channel();
        Self {
            inflight_headers_requests: Default::default(),
            peers: Default::default(),
            queued_requests: Default::default(),
            download_requests_rx: UnboundedReceiverStream::new(download_requests_rx),
            download_requests_tx,
        }
    }
}

/// Front-end API for downloading headers.
#[derive(Debug)]
pub struct HeadersDownloader {
    /// Sender half of the request channel.
    request_tx: UnboundedSender<DownloadRequest>,
}

// === impl HeadersDownloader ===

impl HeadersDownloader {
    /// Sends a `GetBlockHeaders` request to an available peer.
    pub async fn get_block_headers(&self, request: HeadersRequest) -> RequestResult<Vec<Header>> {
        let (response, rx) = oneshot::channel();
        self.request_tx.send(DownloadRequest::GetBlockHeaders { request, response })?;
        rx.await?
    }
}

/// Represents a connected peer
struct Peer {
    /// Identifier for requests.
    request_id: u64,
    /// The state this peer currently resides in.
    state: PeerState,
    /// Best known hash that the peer has
    best_hash: H256,
    /// Best known number the peer has.
    best_number: U256,
}

/// Tracks the state of an individual peer
enum PeerState {
    /// Peer is currently not handling requests and is available.
    Idle,
    /// Peer is handling a `GetBlockHeaders` request.
    GetBlockHeaders,
}

/// A request that waits for a response from the network so it can send it back through the response
/// channel.
struct Request<Req, Resp> {
    request: Req,
    response: oneshot::Sender<Resp>,
    started: Instant,
}

/// Requests that can be sent to the Syncer from a [`HeadersDownloader`]
enum DownloadRequest {
    /// Download the requested headers and send response through channel
    GetBlockHeaders {
        request: HeadersRequest,
        response: oneshot::Sender<RequestResult<Vec<Header>>>,
    },
    /// Download the requested headers and send response through channel
    GetBlockBodies { request: Vec<H256>, response: oneshot::Sender<RequestResult<Vec<BlockBody>>> },
}

/// An action the syncer can emit.
pub(crate) enum FetchAction {
    /// Dispatch an eth request to the given peer.
    EthRequest {
        node_id: NodeId,
        /// The request to send
        request: EthMessage,
    },
}

/// Outcome of a processed response.
///
/// Returned after processing a response.
#[derive(Debug)]
pub(crate) enum BlockResponseOutcome {
    /// Continue with another request to the peer.
    Request(NodeId, BlockRequest),
    /// How to handle a bad response
    // TODO this should include some form of reputation change
    BadResponse(NodeId),
}

//! Blocks/Headers management for the p2p network.

use crate::peers::PeersHandle;
use futures::StreamExt;
use reth_eth_wire::{BlockBodies, BlockBody, BlockHeaders, GetBlockBodies, GetBlockHeaders};
use reth_interfaces::{
    p2p::error::RequestResult,
    provider::{BlockProvider, HeaderProvider},
};
use reth_primitives::{BlockHashOrNumber, Header, PeerId, H256};
use std::{
    borrow::Borrow,
    future::Future,
    hash::Hash,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::sync::{mpsc::UnboundedReceiver, oneshot};
use tokio_stream::wrappers::UnboundedReceiverStream;

// Limits: <https://github.com/ethereum/go-ethereum/blob/b0d44338bbcefee044f1f635a84487cbbd8f0538/eth/protocols/eth/handler.go#L34-L56>

/// Maximum number of block headers to serve.
///
/// Used to limit lookups.
const MAX_HEADERS_SERVE: usize = 1024;

/// Maximum number of block headers to serve.
///
/// Used to limit lookups. With 24KB block sizes nowadays, the practical limit will always be
/// softResponseLimit.
const MAX_BODIES_SERVE: usize = 1024;

/// Maximum size of replies to data retrievals.
const SOFT_RESPONSE_LIMIT: usize = 2 * 1024 * 1024;

/// To punish spam, we answer the same request only a limited number.
const MAX_REPEAT_REQUESTS_PER_PEER: usize = 2;

/// Heuristic limit for recording header requests.
const HEADERS_REQUEST_CACHE_LIMIT: usize = 100;

/// Manages block requests on top of the p2p network.
///
/// This can be spawned to another task and is supposed to be run as background service.
#[must_use = "Manager does nothing unless polled."]
pub struct BlockRequestManager<C> {
    /// The client type that can interact with the chain.
    client: Arc<C>,
    /// Used for reporting peers.
    peers: PeersHandle,
    /// Incoming request from the [`NetworkManager`].
    incoming_requests: UnboundedReceiverStream<IncomingBlockRequest>,
}

// === impl BlockRequestManager ===

impl<C> BlockRequestManager<C>
where
    C: BlockProvider + HeaderProvider,
{
    /// Create a new instance
    pub fn new(
        client: Arc<C>,
        peers: PeersHandle,
        incoming: UnboundedReceiver<IncomingBlockRequest>,
    ) -> Self {
        Self { client, peers, incoming_requests: UnboundedReceiverStream::new(incoming) }
    }

    /// Returns the hash of the block to start with.
    fn get_start_hash(
        &self,
        start_block: BlockHashOrNumber,
        skip: u32,
        reverse: bool,
    ) -> Option<H256> {
        let block_hash = match start_block {
            BlockHashOrNumber::Hash(start) => {
                if skip == 0 {
                    start
                } else {
                    let num = self.client.block_number(start).unwrap_or_default()?;
                    self.get_start_hash(BlockHashOrNumber::Number(num), skip, reverse)?
                }
            }
            BlockHashOrNumber::Number(num) => {
                let num = if reverse {
                    num.checked_sub(skip as u64)?
                } else {
                    num.checked_add(skip as u64)?
                };
                self.client.block_hash(num.into()).unwrap_or_default()?
            }
        };

        Some(block_hash)
    }

    /// Returns the list of requested heders
    fn get_headers_response(&self, request: GetBlockHeaders) -> Vec<Header> {
        let GetBlockHeaders { start_block, limit: _, skip, reverse } = request;

        let mut headers = Vec::new();

        let block_hash = if let Some(hash) = self.get_start_hash(start_block, skip, reverse) {
            hash
        } else {
            return headers
        };

        while let Some(header) = self.client.header(&block_hash).unwrap_or_default() {
            headers.push(header);

            // TODO: size estimation

            if headers.len() >= MAX_HEADERS_SERVE {
                break
            }
        }

        headers
    }

    fn on_headers_request(
        &mut self,
        _peer_id: PeerId,
        request: GetBlockHeaders,
        response: oneshot::Sender<RequestResult<BlockHeaders>>,
    ) {
        let headers = self.get_headers_response(request);

        let _ = response.send(Ok(BlockHeaders(headers)));
    }

    fn on_bodies_request(
        &mut self,
        _peer_id: PeerId,
        request: GetBlockBodies,
        response: oneshot::Sender<RequestResult<BlockBodies>>,
    ) {
        let mut bodies = Vec::new();

        for hash in request.0 {
            if let Some(block) = self.client.block(hash.into()).unwrap_or_default() {
                let body = BlockBody { transactions: block.body, ommers: block.ommers };

                bodies.push(body);

                // TODO: size estimation

                if bodies.len() >= MAX_BODIES_SERVE {
                    break
                }
            } else {
                break
            }
        }

        let _ = response.send(Ok(BlockBodies(bodies)));
    }
}

/// An endless future.
///
/// This should be spawned or used as part of `tokio::select!`.
impl<C> Future for BlockRequestManager<C>
where
    C: BlockProvider + HeaderProvider,
{
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        loop {
            match this.incoming_requests.poll_next_unpin(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(None) => return Poll::Ready(()),
                Poll::Ready(Some(incoming)) => match incoming {
                    IncomingBlockRequest::GetBlockHeaders { peer_id, request, response } => {
                        this.on_headers_request(peer_id, request, response)
                    }
                    IncomingBlockRequest::GetBlockBodies { peer_id, request, response } => {
                        this.on_bodies_request(peer_id, request, response)
                    }
                },
            }
        }
    }
}

/// Represents a handled [`GetBlockHeaders`] requests
///
/// This is the key type for spam detection cache. The counter is ignored during `PartialEq` and
/// `Hash`.
#[derive(Debug, PartialEq, Hash)]
#[allow(unused)]
struct RespondedGetBlockHeaders {
    req: (PeerId, GetBlockHeaders),
}

// === impl RespondedGetBlockHeaders ===

impl RespondedGetBlockHeaders {
    fn new(req: (PeerId, GetBlockHeaders)) -> Self {
        Self { req }
    }
}

impl Borrow<(PeerId, GetBlockHeaders)> for RespondedGetBlockHeaders {
    fn borrow(&self) -> &(PeerId, GetBlockHeaders) {
        &self.req
    }
}

// SAFETY: The [`RespondedGetBlockHeaders`] is only ever accessed mutably via
// [`BlockRequestManager`]
unsafe impl Send for RespondedGetBlockHeaders {}

/// All request related to blocks delegated by the network.
#[derive(Debug)]
#[allow(missing_docs)]
pub enum IncomingBlockRequest {
    /// Request Block headers from the peer.
    ///
    /// The response should be sent through the channel.
    GetBlockHeaders {
        peer_id: PeerId,
        request: GetBlockHeaders,
        response: oneshot::Sender<RequestResult<BlockHeaders>>,
    },
    /// Request Block headers from the peer.
    ///
    /// The response should be sent through the channel.
    GetBlockBodies {
        peer_id: PeerId,
        request: GetBlockBodies,
        response: oneshot::Sender<RequestResult<BlockBodies>>,
    },
}

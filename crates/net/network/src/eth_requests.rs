//! Blocks/Headers management for the p2p network.

use crate::peers::PeersHandle;
use futures::StreamExt;
use reth_eth_wire::{
    BlockBodies, BlockBody, BlockHeaders, GetBlockBodies, GetBlockHeaders, GetNodeData,
    GetReceipts, NodeData, Receipts,
};
use reth_interfaces::{
    p2p::error::RequestResult,
    provider::{BlockProvider, HeaderProvider},
};
use reth_primitives::{BlockHashOrNumber, Header, PeerId};
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
/// SOFT_RESPONSE_LIMIT.
const MAX_BODIES_SERVE: usize = 1024;

/// Estimated size in bytes of an RLP encoded body.
// TODO: check 24kb blocksize assumption
const APPROX_BODY_SIZE: usize = 24 * 1024;

/// Maximum size of replies to data retrievals.
const SOFT_RESPONSE_LIMIT: usize = 2 * 1024 * 1024;

/// Estimated size in bytes of an RLP encoded header.
const APPROX_HEADER_SIZE: usize = 500;

/// Manages eth related requests on top of the p2p network.
///
/// This can be spawned to another task and is supposed to be run as background service.
#[must_use = "Manager does nothing unless polled."]
pub struct EthRequestHandler<C> {
    /// The client type that can interact with the chain.
    client: Arc<C>,
    /// Used for reporting peers.
    #[allow(unused)]
    // TODO use to report spammers
    peers: PeersHandle,
    /// Incoming request from the [`NetworkManager`].
    incoming_requests: UnboundedReceiverStream<IncomingEthRequest>,
}

// === impl EthRequestHandler ===

impl<C> EthRequestHandler<C>
where
    C: BlockProvider + HeaderProvider,
{
    /// Create a new instance
    pub fn new(
        client: Arc<C>,
        peers: PeersHandle,
        incoming: UnboundedReceiver<IncomingEthRequest>,
    ) -> Self {
        Self { client, peers, incoming_requests: UnboundedReceiverStream::new(incoming) }
    }

    /// Returns the list of requested heders
    fn get_headers_response(&self, request: GetBlockHeaders) -> Vec<Header> {
        let GetBlockHeaders { start_block, limit, skip, reverse } = request;

        let direction = HeadersDirection::new(reverse);
        let mut headers = Vec::new();

        let mut block: BlockHashOrNumber = match start_block {
            BlockHashOrNumber::Hash(start) => start.into(),
            BlockHashOrNumber::Number(num) => {
                if let Some(hash) = self.client.block_hash(num.into()).unwrap_or_default() {
                    hash.into()
                } else {
                    return headers
                }
            }
        };

        let skip = skip as u64;
        let mut total_bytes = APPROX_HEADER_SIZE;

        for _ in 0..limit {
            if let Some(header) = self.client.header_by_hash_or_number(block).unwrap_or_default() {
                match direction {
                    HeadersDirection::Rising => {
                        if let Some(next) = (header.number + 1).checked_add(skip) {
                            block = next.into()
                        } else {
                            break
                        }
                    }
                    HeadersDirection::Falling => {
                        if skip > 0 {
                            // prevent under flows for block.number == 0 and `block.number - skip <
                            // 0`
                            if let Some(next) =
                                header.number.checked_sub(1).and_then(|num| num.checked_sub(skip))
                            {
                                block = next.into()
                            } else {
                                break
                            }
                        } else {
                            block = header.parent_hash.into()
                        }
                    }
                }

                headers.push(header);

                if headers.len() >= MAX_HEADERS_SERVE {
                    break
                }

                total_bytes += APPROX_HEADER_SIZE;

                if total_bytes > SOFT_RESPONSE_LIMIT {
                    break
                }
            } else {
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

        let mut total_bytes = APPROX_BODY_SIZE;

        for hash in request.0 {
            if let Some(block) = self.client.block(hash.into()).unwrap_or_default() {
                let body = BlockBody { transactions: block.body, ommers: block.ommers };

                bodies.push(body);

                total_bytes += APPROX_BODY_SIZE;

                if total_bytes > SOFT_RESPONSE_LIMIT {
                    break
                }

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
impl<C> Future for EthRequestHandler<C>
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
                    IncomingEthRequest::GetBlockHeaders { peer_id, request, response } => {
                        this.on_headers_request(peer_id, request, response)
                    }
                    IncomingEthRequest::GetBlockBodies { peer_id, request, response } => {
                        this.on_bodies_request(peer_id, request, response)
                    }
                    IncomingEthRequest::GetNodeData { .. } => {}
                    IncomingEthRequest::GetReceipts { .. } => {}
                },
            }
        }
    }
}

/// Represents the direction for a headers request depending on the `reverse` field of the request.
///
/// [`HeadersDirection::Rising`] block numbers for `reverse == true`
/// [`HeadersDirection::Falling`] block numbers for `reverse == false`
///
/// See also <https://github.com/ethereum/devp2p/blob/master/caps/eth.md#getblockheaders-0x03>
#[derive(Copy, Clone)]
pub enum HeadersDirection {
    /// Rising block number.
    Rising,
    /// Falling block number.
    Falling,
}

impl HeadersDirection {
    fn new(reverse: bool) -> Self {
        if reverse {
            HeadersDirection::Rising
        } else {
            HeadersDirection::Falling
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

impl Borrow<(PeerId, GetBlockHeaders)> for RespondedGetBlockHeaders {
    fn borrow(&self) -> &(PeerId, GetBlockHeaders) {
        &self.req
    }
}

/// All `eth` request related to blocks delegated by the network.
#[derive(Debug)]
#[allow(missing_docs)]
pub enum IncomingEthRequest {
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
    /// Request Node Data from the peer.
    ///
    /// The response should be sent through the channel.
    GetNodeData {
        peer_id: PeerId,
        request: GetNodeData,
        response: oneshot::Sender<RequestResult<NodeData>>,
    },
    /// Request Receipts from the peer.
    ///
    /// The response should be sent through the channel.
    GetReceipts {
        peer_id: PeerId,
        request: GetReceipts,
        response: oneshot::Sender<RequestResult<Receipts>>,
    },
}

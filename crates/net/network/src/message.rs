//! Capability messaging
//!
//! An RLPx stream is multiplexed via the prepended message-id of a framed message.
//! Capabilities are exchanged via the RLPx `Hello` message as pairs of `(id, version)`, <https://github.com/ethereum/devp2p/blob/master/rlpx.md#capability-messaging>

use futures::FutureExt;
use reth_eth_wire::{
    capability::RawCapabilityMessage, message::RequestPair, BlockBodies, BlockBody, BlockHeaders,
    EthMessage, GetBlockBodies, GetBlockHeaders, GetNodeData, GetPooledTransactions, GetReceipts,
    NewBlock, NewBlockHashes, NewPooledTransactionHashes, NodeData, PooledTransactions, Receipts,
    SharedTransactions, Transactions,
};
use reth_interfaces::p2p::error::{RequestError, RequestResult};
use reth_primitives::{Header, PeerId, Receipt, TransactionSigned, H256};
use std::{
    sync::Arc,
    task::{ready, Context, Poll},
};
use tokio::sync::{mpsc, mpsc::error::TrySendError, oneshot};

/// Internal form of a `NewBlock` message
#[derive(Debug, Clone)]
pub struct NewBlockMessage {
    /// Hash of the block
    pub hash: H256,
    /// Raw received message
    pub block: Arc<NewBlock>,
}

// === impl NewBlockMessage ===

impl NewBlockMessage {
    /// Returns the block number of the block
    pub fn number(&self) -> u64 {
        self.block.block.header.number
    }
}

/// All Bi-directional eth-message variants that can be sent to a session or received from a
/// session.
#[derive(Debug)]
pub enum PeerMessage {
    /// Announce new block hashes
    NewBlockHashes(NewBlockHashes),
    /// Broadcast new block.
    NewBlock(NewBlockMessage),
    /// Received transactions _from_ the peer
    ReceivedTransaction(Transactions),
    /// Broadcast transactions _from_ local _to_ a peer.
    SendTransactions(SharedTransactions),
    /// Send new pooled transactions
    PooledTransactions(NewPooledTransactionHashes),
    /// All `eth` request variants.
    EthRequest(PeerRequest),
    /// Other than eth namespace message
    #[allow(unused)]
    Other(RawCapabilityMessage),
}

/// Request Variants that only target block related data.
#[derive(Debug, Clone)]
#[allow(missing_docs)]
#[allow(clippy::enum_variant_names)]
pub enum BlockRequest {
    GetBlockHeaders(GetBlockHeaders),
    GetBlockBodies(GetBlockBodies),
}

/// Protocol related request messages that expect a response
#[derive(Debug)]
#[allow(clippy::enum_variant_names, missing_docs)]
pub enum PeerRequest {
    /// Request Block headers from the peer.
    ///
    /// The response should be sent through the channel.
    GetBlockHeaders {
        request: GetBlockHeaders,
        response: oneshot::Sender<RequestResult<BlockHeaders>>,
    },
    /// Request Block headers from the peer.
    ///
    /// The response should be sent through the channel.
    GetBlockBodies {
        request: GetBlockBodies,
        response: oneshot::Sender<RequestResult<BlockBodies>>,
    },
    /// Request pooled transactions from the peer.
    ///
    /// The response should be sent through the channel.
    GetPooledTransactions {
        request: GetPooledTransactions,
        response: oneshot::Sender<RequestResult<PooledTransactions>>,
    },
    /// Request NodeData from the peer.
    ///
    /// The response should be sent through the channel.
    GetNodeData { request: GetNodeData, response: oneshot::Sender<RequestResult<NodeData>> },
    /// Request Receipts from the peer.
    ///
    /// The response should be sent through the channel.
    GetReceipts { request: GetReceipts, response: oneshot::Sender<RequestResult<Receipts>> },
}

// === impl PeerRequest ===

impl PeerRequest {
    /// Invoked if we received a response which does not match the request
    pub(crate) fn send_bad_response(self) {
        self.send_err_response(RequestError::BadResponse)
    }

    /// Send an error back to the receiver.
    pub(crate) fn send_err_response(self, err: RequestError) {
        let _ = match self {
            PeerRequest::GetBlockHeaders { response, .. } => response.send(Err(err)).ok(),
            PeerRequest::GetBlockBodies { response, .. } => response.send(Err(err)).ok(),
            PeerRequest::GetPooledTransactions { response, .. } => response.send(Err(err)).ok(),
            PeerRequest::GetNodeData { response, .. } => response.send(Err(err)).ok(),
            PeerRequest::GetReceipts { response, .. } => response.send(Err(err)).ok(),
        };
    }

    /// Returns the [`EthMessage`] for this type
    pub fn create_request_message(&self, request_id: u64) -> EthMessage {
        match self {
            PeerRequest::GetBlockHeaders { request, .. } => {
                EthMessage::GetBlockHeaders(RequestPair { request_id, message: *request })
            }
            PeerRequest::GetBlockBodies { request, .. } => {
                EthMessage::GetBlockBodies(RequestPair { request_id, message: request.clone() })
            }
            PeerRequest::GetPooledTransactions { request, .. } => {
                EthMessage::GetPooledTransactions(RequestPair {
                    request_id,
                    message: request.clone(),
                })
            }
            PeerRequest::GetNodeData { request, .. } => {
                EthMessage::GetNodeData(RequestPair { request_id, message: request.clone() })
            }
            PeerRequest::GetReceipts { request, .. } => {
                EthMessage::GetReceipts(RequestPair { request_id, message: request.clone() })
            }
        }
    }
}

/// Corresponding variant for [`PeerRequest`].
#[derive(Debug)]
pub enum PeerResponse {
    BlockHeaders { response: oneshot::Receiver<RequestResult<BlockHeaders>> },
    BlockBodies { response: oneshot::Receiver<RequestResult<BlockBodies>> },
    PooledTransactions { response: oneshot::Receiver<RequestResult<PooledTransactions>> },
    NodeData { response: oneshot::Receiver<RequestResult<NodeData>> },
    Receipts { response: oneshot::Receiver<RequestResult<Receipts>> },
}

// === impl PeerResponse ===

impl PeerResponse {
    /// Polls the type to completion.
    pub(crate) fn poll(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Result<PeerResponseResult, oneshot::error::RecvError>> {
        macro_rules! poll_request {
            ($response:ident, $item:ident, $cx:ident) => {
                match ready!($response.poll_unpin($cx)) {
                    Ok(res) => Ok(PeerResponseResult::$item(res.map(|item| item.0))),
                    Err(err) => Err(err),
                }
            };
        }

        let res = match self {
            PeerResponse::BlockHeaders { response } => {
                poll_request!(response, BlockHeaders, cx)
            }
            PeerResponse::BlockBodies { response } => {
                poll_request!(response, BlockBodies, cx)
            }
            PeerResponse::PooledTransactions { response } => {
                poll_request!(response, PooledTransactions, cx)
            }
            PeerResponse::NodeData { response } => {
                poll_request!(response, NodeData, cx)
            }
            PeerResponse::Receipts { response } => {
                poll_request!(response, Receipts, cx)
            }
        };
        Poll::Ready(res)
    }
}

/// All response variants for [`PeerResponse`]
#[derive(Debug)]
#[allow(missing_docs)]
pub enum PeerResponseResult {
    BlockHeaders(RequestResult<Vec<Header>>),
    BlockBodies(RequestResult<Vec<BlockBody>>),
    PooledTransactions(RequestResult<Vec<TransactionSigned>>),
    NodeData(RequestResult<Vec<bytes::Bytes>>),
    Receipts(RequestResult<Vec<Vec<Receipt>>>),
}

// === impl PeerResponseResult ===

impl PeerResponseResult {
    /// Converts this response into an [`EthMessage`]
    pub fn try_into_message(self, id: u64) -> RequestResult<EthMessage> {
        macro_rules! to_message {
            ($response:ident, $item:ident, $request_id:ident) => {
                match $response {
                    Ok(res) => {
                        let request = RequestPair { request_id: $request_id, message: $item(res) };
                        Ok(EthMessage::$item(request))
                    }
                    Err(err) => Err(err),
                }
            };
        }
        match self {
            PeerResponseResult::BlockHeaders(resp) => {
                to_message!(resp, BlockHeaders, id)
            }
            PeerResponseResult::BlockBodies(resp) => {
                to_message!(resp, BlockBodies, id)
            }
            PeerResponseResult::PooledTransactions(resp) => {
                to_message!(resp, PooledTransactions, id)
            }
            PeerResponseResult::NodeData(resp) => {
                to_message!(resp, NodeData, id)
            }
            PeerResponseResult::Receipts(resp) => {
                to_message!(resp, Receipts, id)
            }
        }
    }

    /// Returns whether this result is an error.
    #[allow(unused)]
    pub fn is_err(&self) -> bool {
        match self {
            PeerResponseResult::BlockHeaders(res) => res.is_err(),
            PeerResponseResult::BlockBodies(res) => res.is_err(),
            PeerResponseResult::PooledTransactions(res) => res.is_err(),
            PeerResponseResult::NodeData(res) => res.is_err(),
            PeerResponseResult::Receipts(res) => res.is_err(),
        }
    }
}

/// A Cloneable connection for sending _requests_ directly to the session of a peer.
#[derive(Debug, Clone)]
pub struct PeerRequestSender {
    /// id of the remote node.
    pub(crate) peer_id: PeerId,
    /// The Sender half connected to a session.
    pub(crate) to_session_tx: mpsc::Sender<PeerRequest>,
}

// === impl PeerRequestSender ===

impl PeerRequestSender {
    /// Attempts to immediately send a message on this Sender
    pub fn try_send(&self, req: PeerRequest) -> Result<(), TrySendError<PeerRequest>> {
        self.to_session_tx.try_send(req)
    }

    /// Returns the peer id of the remote peer.
    pub fn peer_id(&self) -> &PeerId {
        &self.peer_id
    }
}

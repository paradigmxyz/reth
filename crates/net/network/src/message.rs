//! Capability messaging
//!
//! An RLPx stream is multiplexed via the prepended message-id of a framed message.
//! Capabilities are exchanged via the RLPx `Hello` message as pairs of `(id, version)`, <https://github.com/ethereum/devp2p/blob/master/rlpx.md#capability-messaging>

use futures::FutureExt;
use reth_eth_wire::{
    BlockBodies, BlockBody, BlockHeaders, GetBlockBodies, GetBlockHeaders, GetNodeData,
    GetPooledTransactions, GetReceipts, NewBlock, NewBlockHashes, NodeData, PooledTransactions,
    Receipts, Transactions,
};
use std::task::{ready, Context, Poll};

use crate::NodeId;
use reth_eth_wire::capability::CapabilityMessage;
use reth_interfaces::p2p::error::RequestResult;
use reth_primitives::{Header, Receipt, TransactionSigned};
use tokio::sync::{mpsc, mpsc::error::TrySendError, oneshot};

/// Represents all messages that can be sent to a peer session
#[derive(Debug)]
pub enum PeerMessage {
    /// Announce new block hashes
    NewBlockHashes(NewBlockHashes),
    /// Broadcast new block.
    NewBlock(Box<NewBlock>),
    /// Broadcast transactions.
    Transactions(Transactions),
    /// All `eth` request variants.
    EthRequest(PeerRequest),
    /// Other than eth namespace message
    Other(CapabilityMessage),
}

/// Request Variants that only target block related data.
#[derive(Debug, Clone)]
#[allow(missing_docs)]
#[allow(clippy::enum_variant_names)]
pub enum BlockRequest {
    GetBlockHeaders(GetBlockHeaders),
    GetBlockBodies(GetBlockBodies),
}

/// All Request variants of an [`EthMessage`]
///
/// Note: These variants come without a request ID, as it's expected that the peer session will
/// manage those
#[derive(Debug, Clone)]
#[allow(missing_docs)]
#[allow(clippy::enum_variant_names)]
pub enum EthRequest {
    GetBlockHeaders(GetBlockHeaders),
    GetBlockBodies(GetBlockBodies),
    GetPooledTransactions(GetPooledTransactions),
    GetNodeData(GetNodeData),
    GetReceipts(GetReceipts),
}

/// Corresponding Response variants for [`EthRequest`]
#[derive(Debug, Clone)]
#[allow(missing_docs)]
pub enum EthResponse {
    BlockHeaders(BlockHeaders),
    BlockBodies(BlockBodies),
    PooledTransactions(PooledTransactions),
    NodeData(NodeData),
    Receipts(Receipts),
}

/// Protocol related request messages that expect a response
#[derive(Debug)]
#[allow(clippy::enum_variant_names)]
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
    /// Returns whether this result is an error.
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
    pub(crate) peer: NodeId,
    /// The Sender half connected to a session.
    pub(crate) to_session_tx: mpsc::Sender<PeerRequest>,
}

// === impl PeerRequestSender ===

impl PeerRequestSender {
    /// Attempts to immediately send a message on this Sender
    pub fn try_send(&self, req: PeerRequest) -> Result<(), TrySendError<PeerRequest>> {
        self.to_session_tx.try_send(req)
    }
}

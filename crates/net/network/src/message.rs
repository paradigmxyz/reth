//! Capability messaging
//!
//! An `RLPx` stream is multiplexed via the prepended message-id of a framed message.
//! Capabilities are exchanged via the `RLPx` `Hello` message as pairs of `(id, version)`, <https://github.com/ethereum/devp2p/blob/master/rlpx.md#capability-messaging>

use std::{
    sync::Arc,
    task::{ready, Context, Poll},
};

use alloy_primitives::{Bytes, B256};
use futures::FutureExt;
use reth_eth_wire::{
    capability::RawCapabilityMessage, message::RequestPair, BlockBodies, BlockHeaders, EthMessage,
    GetBlockBodies, GetBlockHeaders, NewBlock, NewBlockHashes, NewPooledTransactionHashes,
    NodeData, PooledTransactions, Receipts, SharedTransactions, Transactions,
};
use reth_network_api::PeerRequest;
use reth_network_p2p::error::{RequestError, RequestResult};
use reth_primitives::{BlockBody, Header, PooledTransactionsElement, ReceiptWithBloom};
use tokio::sync::oneshot;

/// Internal form of a `NewBlock` message
#[derive(Debug, Clone)]
pub struct NewBlockMessage {
    /// Hash of the block
    pub hash: B256,
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
    Other(RawCapabilityMessage),
}

/// Request Variants that only target block related data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockRequest {
    /// Requests block headers from the peer.
    ///
    /// The response should be sent through the channel.
    GetBlockHeaders(GetBlockHeaders),

    /// Requests block bodies from the peer.
    ///
    /// The response should be sent through the channel.
    GetBlockBodies(GetBlockBodies),
}

/// Corresponding variant for [`PeerRequest`].
#[derive(Debug)]
pub enum PeerResponse {
    /// Represents a response to a request for block headers.
    BlockHeaders {
        /// The receiver channel for the response to a block headers request.
        response: oneshot::Receiver<RequestResult<BlockHeaders>>,
    },
    /// Represents a response to a request for block bodies.
    BlockBodies {
        /// The receiver channel for the response to a block bodies request.
        response: oneshot::Receiver<RequestResult<BlockBodies>>,
    },
    /// Represents a response to a request for pooled transactions.
    PooledTransactions {
        /// The receiver channel for the response to a pooled transactions request.
        response: oneshot::Receiver<RequestResult<PooledTransactions>>,
    },
    /// Represents a response to a request for `NodeData`.
    NodeData {
        /// The receiver channel for the response to a `NodeData` request.
        response: oneshot::Receiver<RequestResult<NodeData>>,
    },
    /// Represents a response to a request for receipts.
    Receipts {
        /// The receiver channel for the response to a receipts request.
        response: oneshot::Receiver<RequestResult<Receipts>>,
    },
}

// === impl PeerResponse ===

impl PeerResponse {
    /// Polls the type to completion.
    pub(crate) fn poll(&mut self, cx: &mut Context<'_>) -> Poll<PeerResponseResult> {
        macro_rules! poll_request {
            ($response:ident, $item:ident, $cx:ident) => {
                match ready!($response.poll_unpin($cx)) {
                    Ok(res) => PeerResponseResult::$item(res.map(|item| item.0)),
                    Err(err) => PeerResponseResult::$item(Err(err.into())),
                }
            };
        }

        let res = match self {
            Self::BlockHeaders { response } => {
                poll_request!(response, BlockHeaders, cx)
            }
            Self::BlockBodies { response } => {
                poll_request!(response, BlockBodies, cx)
            }
            Self::PooledTransactions { response } => {
                poll_request!(response, PooledTransactions, cx)
            }
            Self::NodeData { response } => {
                poll_request!(response, NodeData, cx)
            }
            Self::Receipts { response } => {
                poll_request!(response, Receipts, cx)
            }
        };
        Poll::Ready(res)
    }
}

/// All response variants for [`PeerResponse`]
#[derive(Debug)]
pub enum PeerResponseResult {
    /// Represents a result containing block headers or an error.
    BlockHeaders(RequestResult<Vec<Header>>),
    /// Represents a result containing block bodies or an error.
    BlockBodies(RequestResult<Vec<BlockBody>>),
    /// Represents a result containing pooled transactions or an error.
    PooledTransactions(RequestResult<Vec<PooledTransactionsElement>>),
    /// Represents a result containing node data or an error.
    NodeData(RequestResult<Vec<Bytes>>),
    /// Represents a result containing receipts or an error.
    Receipts(RequestResult<Vec<Vec<ReceiptWithBloom>>>),
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
            Self::BlockHeaders(resp) => {
                to_message!(resp, BlockHeaders, id)
            }
            Self::BlockBodies(resp) => {
                to_message!(resp, BlockBodies, id)
            }
            Self::PooledTransactions(resp) => {
                to_message!(resp, PooledTransactions, id)
            }
            Self::NodeData(resp) => {
                to_message!(resp, NodeData, id)
            }
            Self::Receipts(resp) => {
                to_message!(resp, Receipts, id)
            }
        }
    }

    /// Returns the `Err` value if the result is an error.
    pub fn err(&self) -> Option<&RequestError> {
        match self {
            Self::BlockHeaders(res) => res.as_ref().err(),
            Self::BlockBodies(res) => res.as_ref().err(),
            Self::PooledTransactions(res) => res.as_ref().err(),
            Self::NodeData(res) => res.as_ref().err(),
            Self::Receipts(res) => res.as_ref().err(),
        }
    }

    /// Returns whether this result is an error.
    pub fn is_err(&self) -> bool {
        self.err().is_some()
    }
}

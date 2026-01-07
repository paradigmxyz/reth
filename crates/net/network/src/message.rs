//! Capability messaging
//!
//! An `RLPx` stream is multiplexed via the prepended message-id of a framed message.
//! Capabilities are exchanged via the `RLPx` `Hello` message as pairs of `(id, version)`, <https://github.com/ethereum/devp2p/blob/master/rlpx.md#capability-messaging>

use crate::types::{Receipts69, Receipts70};
use alloy_consensus::{BlockHeader, ReceiptWithBloom};
use alloy_primitives::{Bytes, B256};
use futures::FutureExt;
use reth_eth_wire::{
    eth_snap_stream::EthSnapMessage, message::RequestPair, BlockBodies, BlockHeaders,
    BlockRangeUpdate, EthMessage, EthNetworkPrimitives, GetBlockBodies, GetBlockHeaders,
    NetworkPrimitives, NewBlock, NewBlockHashes, NewBlockPayload, NewPooledTransactionHashes,
    NodeData, PooledTransactions, Receipts, SharedTransactions, Transactions,
};
use reth_eth_wire_types::{
    snap::{
        AccountRangeMessage, ByteCodesMessage, SnapProtocolMessage, StorageRangesMessage,
        TrieNodesMessage,
    },
    RawCapabilityMessage,
};
use reth_network_api::PeerRequest;
use reth_network_p2p::error::{RequestError, RequestResult};
use reth_primitives_traits::Block;
use std::{
    sync::Arc,
    task::{ready, Context, Poll},
};
use tokio::sync::oneshot;

/// Internal form of a `NewBlock` message
#[derive(Debug, Clone)]
pub struct NewBlockMessage<P = NewBlock<reth_ethereum_primitives::Block>> {
    /// Hash of the block
    pub hash: B256,
    /// Raw received message
    pub block: Arc<P>,
}

// === impl NewBlockMessage ===

impl<P: NewBlockPayload> NewBlockMessage<P> {
    /// Returns the block number of the block
    pub fn number(&self) -> u64 {
        self.block.block().header().number()
    }
}

/// All Bi-directional eth-message variants that can be sent to a session or received from a
/// session.
#[derive(Debug)]
pub enum PeerMessage<N: NetworkPrimitives = EthNetworkPrimitives> {
    /// Announce new block hashes
    NewBlockHashes(NewBlockHashes),
    /// Broadcast new block.
    NewBlock(NewBlockMessage<N::NewBlockPayload>),
    /// Received transactions _from_ the peer
    ReceivedTransaction(Transactions<N::BroadcastedTransaction>),
    /// Broadcast transactions _from_ local _to_ a peer.
    SendTransactions(SharedTransactions<N::BroadcastedTransaction>),
    /// Send new pooled transactions
    PooledTransactions(NewPooledTransactionHashes),
    /// All protocol request variants (`eth` and `snap` wrapped in `PeerRequest`).
    EthRequest(PeerRequest<N>),
    /// Announces when `BlockRange` is updated.
    BlockRangeUpdated(BlockRangeUpdate),
    /// Any other or manually crafted eth message.
    ///
    /// Caution: It is expected that this is a valid `eth_` capability message.
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
pub enum PeerResponse<N: NetworkPrimitives = EthNetworkPrimitives> {
    /// Represents a response to a request for block headers.
    BlockHeaders {
        /// The receiver channel for the response to a block headers request.
        response: oneshot::Receiver<RequestResult<BlockHeaders<N::BlockHeader>>>,
    },
    /// Represents a response to a request for block bodies.
    BlockBodies {
        /// The receiver channel for the response to a block bodies request.
        response: oneshot::Receiver<RequestResult<BlockBodies<N::BlockBody>>>,
    },
    /// Represents a response to a request for pooled transactions.
    PooledTransactions {
        /// The receiver channel for the response to a pooled transactions request.
        response: oneshot::Receiver<RequestResult<PooledTransactions<N::PooledTransaction>>>,
    },
    /// Represents a response to a request for `NodeData`.
    NodeData {
        /// The receiver channel for the response to a `NodeData` request.
        response: oneshot::Receiver<RequestResult<NodeData>>,
    },
    /// Represents a response to a request for receipts.
    Receipts {
        /// The receiver channel for the response to a receipts request.
        response: oneshot::Receiver<RequestResult<Receipts<N::Receipt>>>,
    },
    /// Represents a response to a request for receipts.
    ///
    /// This is a variant of `Receipts` that was introduced in `eth/69`.
    /// The difference is that this variant does not require the inclusion of bloom filters in the
    /// response, making it more lightweight.
    Receipts69 {
        /// The receiver channel for the response to a receipts request.
        response: oneshot::Receiver<RequestResult<Receipts69<N::Receipt>>>,
    },
    /// Represents a response to a request for receipts using eth/70.
    Receipts70 {
        /// The receiver channel for the response to a receipts request.
        response: oneshot::Receiver<RequestResult<Receipts70<N::Receipt>>>,
    },
    /// Snap account range response.
    SnapAccountRange {
        /// Receiver for the account range response.
        response: oneshot::Receiver<RequestResult<AccountRangeMessage>>,
    },
    /// Snap storage ranges response.
    SnapStorageRanges {
        /// Receiver for the storage ranges response.
        response: oneshot::Receiver<RequestResult<StorageRangesMessage>>,
    },
    /// Snap byte codes response.
    SnapByteCodes {
        /// Receiver for the byte codes response.
        response: oneshot::Receiver<RequestResult<ByteCodesMessage>>,
    },
    /// Snap trie nodes response.
    SnapTrieNodes {
        /// Receiver for the trie nodes response.
        response: oneshot::Receiver<RequestResult<TrieNodesMessage>>,
    },
}

// === impl PeerResponse ===

impl<N: NetworkPrimitives> PeerResponse<N> {
    /// Polls the type to completion.
    pub(crate) fn poll(&mut self, cx: &mut Context<'_>) -> Poll<PeerResponseResult<N>> {
        macro_rules! poll_request {
            ($response:ident, $item:ident, $cx:ident) => {
                match ready!($response.poll_unpin($cx)) {
                    Ok(res) => PeerResponseResult::$item(res.map(|item| item.0)),
                    Err(err) => PeerResponseResult::$item(Err(err.into())),
                }
            };
        }

        let res = match self {
            Self::BlockHeaders { response } => poll_request!(response, BlockHeaders, cx),
            Self::BlockBodies { response } => poll_request!(response, BlockBodies, cx),
            Self::PooledTransactions { response } => {
                poll_request!(response, PooledTransactions, cx)
            }
            Self::NodeData { response } => {
                poll_request!(response, NodeData, cx)
            }
            Self::Receipts { response } => {
                poll_request!(response, Receipts, cx)
            }
            Self::Receipts69 { response } => {
                poll_request!(response, Receipts69, cx)
            }
            Self::Receipts70 { response } => match ready!(response.poll_unpin(cx)) {
                Ok(res) => PeerResponseResult::Receipts70(res),
                Err(err) => PeerResponseResult::Receipts70(Err(err.into())),
            },
            Self::SnapAccountRange { response } => match ready!(response.poll_unpin(cx)) {
                Ok(res) => PeerResponseResult::SnapAccountRange(res),
                Err(err) => PeerResponseResult::SnapAccountRange(Err(err.into())),
            },
            Self::SnapStorageRanges { response } => match ready!(response.poll_unpin(cx)) {
                Ok(res) => PeerResponseResult::SnapStorageRanges(res),
                Err(err) => PeerResponseResult::SnapStorageRanges(Err(err.into())),
            },
            Self::SnapByteCodes { response } => match ready!(response.poll_unpin(cx)) {
                Ok(res) => PeerResponseResult::SnapByteCodes(res),
                Err(err) => PeerResponseResult::SnapByteCodes(Err(err.into())),
            },
            Self::SnapTrieNodes { response } => match ready!(response.poll_unpin(cx)) {
                Ok(res) => PeerResponseResult::SnapTrieNodes(res),
                Err(err) => PeerResponseResult::SnapTrieNodes(Err(err.into())),
            },
        };
        Poll::Ready(res)
    }
}

/// All response variants for [`PeerResponse`]
#[derive(Debug)]
pub enum PeerResponseResult<N: NetworkPrimitives = EthNetworkPrimitives> {
    /// Represents a result containing block headers or an error.
    BlockHeaders(RequestResult<Vec<N::BlockHeader>>),
    /// Represents a result containing block bodies or an error.
    BlockBodies(RequestResult<Vec<N::BlockBody>>),
    /// Represents a result containing pooled transactions or an error.
    PooledTransactions(RequestResult<Vec<N::PooledTransaction>>),
    /// Represents a result containing node data or an error.
    NodeData(RequestResult<Vec<Bytes>>),
    /// Represents a result containing receipts or an error.
    Receipts(RequestResult<Vec<Vec<ReceiptWithBloom<N::Receipt>>>>),
    /// Represents a result containing receipts or an error for eth/69.
    Receipts69(RequestResult<Vec<Vec<N::Receipt>>>),
    /// Represents a result containing receipts or an error for eth/70.
    Receipts70(RequestResult<Receipts70<N::Receipt>>),
    /// Snap account range response.
    SnapAccountRange(RequestResult<AccountRangeMessage>),
    /// Snap storage ranges response.
    SnapStorageRanges(RequestResult<StorageRangesMessage>),
    /// Snap bytecodes response.
    SnapByteCodes(RequestResult<ByteCodesMessage>),
    /// Snap trie nodes response.
    SnapTrieNodes(RequestResult<TrieNodesMessage>),
}

macro_rules! to_message {
    ($response:expr, $item:ident, $request_id:expr, $wrap:expr) => {
        match $response {
            Ok(res) => {
                let request = RequestPair { request_id: $request_id, message: $item(res) };
                Ok($wrap(EthMessage::$item(request)))
            }
            Err(err) => Err(err),
        }
    };
}

// === impl PeerResponseResult ===

impl<N: NetworkPrimitives> PeerResponseResult<N> {
    /// Converts this response into an [`EthMessage`]
    pub fn try_into_message(self, id: u64) -> RequestResult<EthMessage<N>> {
        match self {
            Self::BlockHeaders(resp) => {
                to_message!(resp, BlockHeaders, id, |message| message)
            }
            Self::BlockBodies(resp) => {
                to_message!(resp, BlockBodies, id, |message| message)
            }
            Self::PooledTransactions(resp) => {
                to_message!(resp, PooledTransactions, id, |message| message)
            }
            Self::NodeData(resp) => {
                to_message!(resp, NodeData, id, |message| message)
            }
            Self::Receipts(resp) => {
                to_message!(resp, Receipts, id, |message| message)
            }
            Self::Receipts69(resp) => {
                to_message!(resp, Receipts69, id, |message| message)
            }
            Self::Receipts70(resp) => match resp {
                Ok(res) => {
                    let request = RequestPair { request_id: id, message: res };
                    Ok(EthMessage::Receipts70(request))
                }
                Err(err) => Err(err),
            },
            // Snap responses cannot be represented as `EthMessage` directly.
            // Callers that need snap support should use `try_into_eth_snap_message` instead.
            Self::SnapAccountRange(resp) => match resp {
                Ok(_) => Err(RequestError::UnsupportedCapability),
                Err(err) => Err(err),
            },
            Self::SnapStorageRanges(resp) => match resp {
                Ok(_) => Err(RequestError::UnsupportedCapability),
                Err(err) => Err(err),
            },
            Self::SnapByteCodes(resp) => match resp {
                Ok(_) => Err(RequestError::UnsupportedCapability),
                Err(err) => Err(err),
            },
            Self::SnapTrieNodes(resp) => match resp {
                Ok(_) => Err(RequestError::UnsupportedCapability),
                Err(err) => Err(err),
            },
        }
    }

    /// Converts this response into an [`EthSnapMessage`].
    pub fn try_into_eth_snap_message(self, id: u64) -> RequestResult<EthSnapMessage<N>> {
        match self {
            Self::BlockHeaders(resp) => {
                to_message!(resp, BlockHeaders, id, EthSnapMessage::Eth)
            }
            Self::BlockBodies(resp) => {
                to_message!(resp, BlockBodies, id, EthSnapMessage::Eth)
            }
            Self::PooledTransactions(resp) => {
                to_message!(resp, PooledTransactions, id, EthSnapMessage::Eth)
            }
            Self::NodeData(resp) => {
                to_message!(resp, NodeData, id, EthSnapMessage::Eth)
            }
            Self::Receipts(resp) => {
                to_message!(resp, Receipts, id, EthSnapMessage::Eth)
            }
            Self::Receipts69(resp) => {
                to_message!(resp, Receipts69, id, EthSnapMessage::Eth)
            }
            Self::Receipts70(resp) => match resp {
                Ok(res) => {
                    let request = RequestPair { request_id: id, message: res };
                    Ok(EthSnapMessage::Eth(EthMessage::Receipts70(request)))
                }
                Err(err) => Err(err),
            },
            Self::SnapAccountRange(resp) => match resp {
                Ok(resp) => Ok(EthSnapMessage::Snap(SnapProtocolMessage::AccountRange(resp))),
                Err(err) => Err(err),
            },
            Self::SnapStorageRanges(resp) => match resp {
                Ok(resp) => Ok(EthSnapMessage::Snap(SnapProtocolMessage::StorageRanges(resp))),
                Err(err) => Err(err),
            },
            Self::SnapByteCodes(resp) => match resp {
                Ok(resp) => Ok(EthSnapMessage::Snap(SnapProtocolMessage::ByteCodes(resp))),
                Err(err) => Err(err),
            },
            Self::SnapTrieNodes(resp) => match resp {
                Ok(resp) => Ok(EthSnapMessage::Snap(SnapProtocolMessage::TrieNodes(resp))),
                Err(err) => Err(err),
            },
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
            Self::Receipts69(res) => res.as_ref().err(),
            Self::Receipts70(res) => res.as_ref().err(),
            Self::SnapAccountRange(res) => res.as_ref().err(),
            Self::SnapStorageRanges(res) => res.as_ref().err(),
            Self::SnapByteCodes(res) => res.as_ref().err(),
            Self::SnapTrieNodes(res) => res.as_ref().err(),
        }
    }

    /// Returns whether this result is an error.
    pub fn is_err(&self) -> bool {
        self.err().is_some()
    }
}

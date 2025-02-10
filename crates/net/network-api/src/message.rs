use alloy_rlp::Encodable;
use derive_more::Debug;
use reth_eth_wire_types::{
    message::RequestPair, BlockBodies, BlockHeaders, EthMessage, GetBlockBodies, GetBlockHeaders,
    GetNodeData, GetPooledTransactions, GetReceipts, NetworkPrimitives, NodeData,
    PooledTransactions, Receipts,
};

/// Represents a protocol message in a generic networking stack.
pub trait NetworkMessage<N: NetworkPrimitives>:
    Debug + Clone + Send + Sync + Encodable + Unpin + 'static
{
    /// Returns a unique identifier for the message.
    fn message_id(&self) -> u8;

    /// Returns `true` if the message is a request.
    fn is_request(&self) -> bool;

    /// Returns `true` if the message is a response.
    fn is_response(&self) -> bool;

    // TODO: Split me into multiple Request and Response trait and extend this trait

    /// Creates a new `GetBlockHeaders` message.    
    fn get_block_headers(pair: RequestPair<GetBlockHeaders>) -> Self;

    /// Creates a new `GetBlockBodies` message.
    fn get_block_bodies(pair: RequestPair<GetBlockBodies>) -> Self;

    /// Creates a new `GetPooledTransactions` message.
    fn get_pooled_transactions(pair: RequestPair<GetPooledTransactions>) -> Self;

    /// Creates a new `GetNodeData` message.
    fn get_node_data(pair: RequestPair<GetNodeData>) -> Self;

    /// Creates a new `GetReceipts` message.
    fn get_receipts(pair: RequestPair<GetReceipts>) -> Self;

    /// Creates a new `BlockHeaders` message.
    fn block_headers(pair: RequestPair<BlockHeaders<N::BlockHeader>>) -> Self;

    /// Creates a new `BlockBodies` message.
    fn block_bodies(pair: RequestPair<BlockBodies<N::BlockBody>>) -> Self;

    /// Creates a new `PooledTransactions` message.
    fn pooled_transactions(pair: RequestPair<PooledTransactions<N::PooledTransaction>>) -> Self;

    /// Creates a new `NodeData` message.
    fn node_data(pair: RequestPair<NodeData>) -> Self;

    /// Creates a new `Receipts` message.
    fn receipts(pair: RequestPair<Receipts<N::Receipt>>) -> Self;
}

impl<N: NetworkPrimitives> NetworkMessage<N> for EthMessage<N> {
    fn message_id(&self) -> u8 {
        self.message_id() as u8
    }

    fn is_request(&self) -> bool {
        self.is_request()
    }

    fn is_response(&self) -> bool {
        self.is_response()
    }

    // Requests
    fn get_block_headers(pair: RequestPair<GetBlockHeaders>) -> Self {
        Self::GetBlockHeaders(pair)
    }

    fn get_block_bodies(pair: RequestPair<GetBlockBodies>) -> Self {
        Self::GetBlockBodies(pair)
    }

    fn get_pooled_transactions(pair: RequestPair<GetPooledTransactions>) -> Self {
        Self::GetPooledTransactions(pair)
    }

    fn get_node_data(pair: RequestPair<GetNodeData>) -> Self {
        Self::GetNodeData(pair)
    }

    fn get_receipts(pair: RequestPair<GetReceipts>) -> Self {
        Self::GetReceipts(pair)
    }

    // Responses
    fn block_headers(pair: RequestPair<BlockHeaders<N::BlockHeader>>) -> Self {
        Self::BlockHeaders(pair)
    }

    fn block_bodies(pair: RequestPair<BlockBodies<N::BlockBody>>) -> Self {
        Self::BlockBodies(pair)
    }

    fn pooled_transactions(pair: RequestPair<PooledTransactions<N::PooledTransaction>>) -> Self {
        Self::PooledTransactions(pair)
    }

    fn node_data(pair: RequestPair<NodeData>) -> Self {
        Self::NodeData(pair)
    }

    fn receipts(pair: RequestPair<Receipts<N::Receipt>>) -> Self {
        Self::Receipts(pair)
    }
}

//! Types for broadcasting new data.
use reth_primitives::{Header, TransactionSigned, H256, U128};
use reth_rlp::{RlpDecodable, RlpDecodableWrapper, RlpEncodable, RlpEncodableWrapper};

/// This informs peers of new blocks that have appeared on the network.
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodableWrapper, RlpDecodableWrapper)]
pub struct NewBlockHashes(
    /// New block hashes and the block number for each blockhash.
    /// Clients should request blocks using a [`GetBlockBodies`](crate::GetBlockBodies) message.
    pub Vec<BlockHashNumber>,
);

/// A block hash _and_ a block number.
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodable, RlpDecodable)]
pub struct BlockHashNumber {
    /// The block hash
    pub hash: H256,
    /// The block number
    pub number: u64,
}

impl From<Vec<BlockHashNumber>> for NewBlockHashes {
    fn from(v: Vec<BlockHashNumber>) -> Self {
        NewBlockHashes(v)
    }
}

impl From<NewBlockHashes> for Vec<BlockHashNumber> {
    fn from(v: NewBlockHashes) -> Self {
        v.0
    }
}

/// A block body, including transactions and uncle headers.
#[derive(Debug, Clone, PartialEq, Eq, Default, RlpEncodable, RlpDecodable)]
pub struct RawBlockBody {
    /// This block's header
    pub header: Header,
    /// Transactions in this block.
    pub transactions: Vec<TransactionSigned>,
    /// Uncle block headers.
    pub ommers: Vec<Header>,
}

/// A new block with the current total difficulty, which includes the difficulty of the returned
/// block.
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodable, RlpDecodable)]
pub struct NewBlock {
    /// A new block.
    pub block: RawBlockBody,
    /// The current total difficulty.
    pub td: U128,
}

/// This informs peers of transactions that have appeared on the network and are not yet included
/// in a block.
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodableWrapper, RlpDecodableWrapper)]
pub struct Transactions(
    /// New transactions for the peer to include in its mempool.
    pub Vec<TransactionSigned>,
);

impl From<Vec<TransactionSigned>> for Transactions {
    fn from(txs: Vec<TransactionSigned>) -> Self {
        Transactions(txs)
    }
}

impl From<Transactions> for Vec<TransactionSigned> {
    fn from(txs: Transactions) -> Self {
        txs.0
    }
}

/// This informs peers of transaction hashes for transactions that have appeared on the network,
/// but have not been included in a block.
#[derive(Clone, Debug, PartialEq, Eq, RlpEncodableWrapper, RlpDecodableWrapper)]
pub struct NewPooledTransactionHashes(
    /// Transaction hashes for new transactions that have appeared on the network.
    /// Clients should request the transactions with the given hashes using a
    /// [`GetPooledTransactions`](crate::GetPooledTransactions) message.
    pub Vec<H256>,
);

impl From<Vec<H256>> for NewPooledTransactionHashes {
    fn from(v: Vec<H256>) -> Self {
        NewPooledTransactionHashes(v)
    }
}

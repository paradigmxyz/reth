//! Types for broadcasting new data.
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
    pub hash: reth_primitives::H256,
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

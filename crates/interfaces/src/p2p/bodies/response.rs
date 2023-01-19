use derive_more::{Deref, From};
use reth_primitives::{BlockNumber, SealedBlock, SealedHeader};
use std::cmp::Ordering;

/// The block response
#[derive(PartialEq, Eq, Debug)]
pub enum BlockResponse {
    /// Full block response (with transactions or ommers)
    Full(SealedBlock),
    /// The empty block response
    Empty(SealedHeader),
}

impl BlockResponse {
    /// Return the reference to the response header
    pub fn header(&self) -> &SealedHeader {
        match self {
            BlockResponse::Full(block) => &block.header,
            BlockResponse::Empty(header) => header,
        }
    }

    /// Return the block number
    pub fn block_number(&self) -> BlockNumber {
        self.header().number
    }
}

/// Ordered block response.
#[derive(Deref, From, Debug)]
pub struct OrderedBlockResponse(
    /// Inner block response
    pub BlockResponse,
);

impl PartialEq for OrderedBlockResponse {
    fn eq(&self, other: &Self) -> bool {
        self.block_number() == other.block_number()
    }
}

impl Eq for OrderedBlockResponse {}

impl PartialOrd for OrderedBlockResponse {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OrderedBlockResponse {
    fn cmp(&self, other: &Self) -> Ordering {
        self.block_number().cmp(&other.block_number())
    }
}

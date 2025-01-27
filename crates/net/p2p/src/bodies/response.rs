use alloy_consensus::BlockHeader;
use alloy_primitives::{BlockNumber, U256};
use reth_primitives::{SealedBlock, SealedHeader};
use reth_primitives_traits::{Block, InMemorySize};
/// The block response
#[derive(PartialEq, Eq, Debug, Clone)]
pub enum BlockResponse<B: Block> {
    /// Full block response (with transactions or ommers)
    Full(SealedBlock<B>),
    /// The empty block response
    Empty(SealedHeader<B::Header>),
}

impl<B> BlockResponse<B>
where
    B: Block,
{
    /// Return the block number
    pub fn block_number(&self) -> BlockNumber {
        match self {
            Self::Full(block) => block.number(),
            Self::Empty(header) => header.number(),
        }
    }

    /// Return the reference to the response header
    pub fn difficulty(&self) -> U256 {
        match self {
            Self::Full(block) => block.difficulty(),
            Self::Empty(header) => header.difficulty(),
        }
    }

    /// Return the reference to the response body
    pub fn into_body(self) -> Option<B::Body> {
        match self {
            Self::Full(block) => Some(block.into_body()),
            Self::Empty(_) => None,
        }
    }
}

impl<B: Block> InMemorySize for BlockResponse<B> {
    #[inline]
    fn size(&self) -> usize {
        match self {
            Self::Full(block) => SealedBlock::size(block),
            Self::Empty(header) => SealedHeader::size(header),
        }
    }
}

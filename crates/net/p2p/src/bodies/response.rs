use alloy_consensus::BlockHeader;
use alloy_primitives::{BlockNumber, U256};
use reth_primitives::{BlockBody, SealedBlock, SealedHeader};
use reth_primitives_traits::InMemorySize;
/// The block response
#[derive(PartialEq, Eq, Debug, Clone)]
pub enum BlockResponse<H, B = BlockBody> {
    /// Full block response (with transactions or ommers)
    Full(SealedBlock<H, B>),
    /// The empty block response
    Empty(SealedHeader<H>),
}

impl<H, B> BlockResponse<H, B>
where
    H: BlockHeader,
{
    /// Return the reference to the response header
    pub const fn header(&self) -> &SealedHeader<H> {
        match self {
            Self::Full(block) => block.sealed_header(),
            Self::Empty(header) => header,
        }
    }

    /// Return the block number
    pub fn block_number(&self) -> BlockNumber {
        self.header().number()
    }

    /// Return the reference to the response header
    pub fn difficulty(&self) -> U256 {
        match self {
            Self::Full(block) => block.difficulty(),
            Self::Empty(header) => header.difficulty(),
        }
    }

    /// Return the reference to the response body
    pub fn into_body(self) -> Option<B> {
        match self {
            Self::Full(block) => Some(block.into_body()),
            Self::Empty(_) => None,
        }
    }
}

impl<H: InMemorySize, B: InMemorySize> InMemorySize for BlockResponse<H, B> {
    #[inline]
    fn size(&self) -> usize {
        match self {
            Self::Full(block) => SealedBlock::size(block),
            Self::Empty(header) => SealedHeader::size(header),
        }
    }
}

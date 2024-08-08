use std::mem;

use reth_eth_wire_types::{types::BlockHeader, NetworkTypes, PrimitiveNetworkTypes};
use reth_primitives::{alloy_primitives::Sealed, BlockNumber, SealedBlock, B256};

/// The block response
#[derive(PartialEq, Eq, Debug, Clone)]
pub enum BlockResponse<T: NetworkTypes = PrimitiveNetworkTypes> {
    /// Full block response (with transactions or ommers)
    Full(SealedBlock),
    /// The empty block response
    Empty(Sealed<T::BlockHeader>),
}

impl<T: NetworkTypes> BlockResponse<T> {
    /// Calculates a heuristic for the in-memory size of the [`BlockResponse`].
    #[inline]
    pub fn size(&self) -> usize {
        match self {
            Self::Full(block) => block.size(),
            Self::Empty(header) => header.size() + mem::size_of::<B256>(),
        }
    }

    /// Return the block number
    pub fn block_number(&self) -> BlockNumber {
        match self {
            Self::Full(block) => block.header().number,
            Self::Empty(header) => header.number(),
        }
    }
}

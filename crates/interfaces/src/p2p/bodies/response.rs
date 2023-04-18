use reth_primitives::{BlockNumber, SealedBlock, SealedHeader, U256};

/// The block response
#[derive(PartialEq, Eq, Debug, Clone)]
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

    /// Return the reference to the response header
    pub fn difficulty(&self) -> U256 {
        match self {
            BlockResponse::Full(block) => block.difficulty,
            BlockResponse::Empty(header) => header.difficulty,
        }
    }
}

use crate::Result;
use auto_impl::auto_impl;
use reth_primitives::{
    rpc::{BlockId, BlockNumber},
    Block, BlockHash, BlockHashOrNumber, Header, H256, U256,
};

/// Client trait for fetching `Header` related data.
#[auto_impl(&)]
pub trait HeaderProvider: Send + Sync {
    /// Check if block is known
    fn is_known(&self, block_hash: &BlockHash) -> Result<bool> {
        self.header(block_hash).map(|header| header.is_some())
    }

    /// Get header by block hash
    fn header(&self, block_hash: &BlockHash) -> Result<Option<Header>>;

    /// Get header by block number
    fn header_by_number(&self, num: u64) -> Result<Option<Header>>;

    /// Get header by block number or hash
    fn header_by_hash_or_number(&self, hash_or_num: BlockHashOrNumber) -> Result<Option<Header>> {
        match hash_or_num {
            BlockHashOrNumber::Hash(hash) => self.header(&hash),
            BlockHashOrNumber::Number(num) => self.header_by_number(num),
        }
    }
}

/// Api trait for fetching `Block` related data.
pub trait BlockProvider: Send + Sync {
    /// Returns the current info for the chain.
    fn chain_info(&self) -> Result<ChainInfo>;

    /// Returns the block. Returns `None` if block is not found.
    fn block(&self, id: BlockId) -> Result<Option<Block>>;

    /// Converts the `BlockNumber` variants.
    fn convert_block_number(
        &self,
        num: BlockNumber,
    ) -> Result<Option<reth_primitives::BlockNumber>> {
        let num = match num {
            BlockNumber::Latest => self.chain_info()?.best_number,
            BlockNumber::Earliest => 0,
            BlockNumber::Pending => return Ok(None),
            BlockNumber::Number(num) => num.as_u64(),
            BlockNumber::Finalized => return Ok(self.chain_info()?.last_finalized),
            BlockNumber::Safe => return Ok(self.chain_info()?.safe_finalized),
        };
        Ok(Some(num))
    }

    /// Get the hash of the block by matching the given id.
    fn block_hash_for_id(&self, block_id: BlockId) -> Result<Option<H256>> {
        match block_id {
            BlockId::Hash(hash) => Ok(Some(hash)),
            BlockId::Number(num) => {
                if matches!(num, BlockNumber::Latest) {
                    return Ok(Some(self.chain_info()?.best_hash))
                }
                self.convert_block_number(num)?
                    .map(|num| self.block_hash(num.into()))
                    .transpose()
                    .map(|maybe_hash| maybe_hash.flatten())
            }
        }
    }

    /// Get the number of the block by matching the given id.
    fn block_number_for_id(
        &self,
        block_id: BlockId,
    ) -> Result<Option<reth_primitives::BlockNumber>> {
        match block_id {
            BlockId::Hash(hash) => self.block_number(hash),
            BlockId::Number(num) => self.convert_block_number(num),
        }
    }

    /// Gets the `Block` for the given hash. Returns `None` if no block with this hash exists.
    fn block_number(&self, hash: H256) -> Result<Option<reth_primitives::BlockNumber>>;

    /// Get the hash of the block with the given number. Returns `None` if no block with this number
    /// exists.
    fn block_hash(&self, number: U256) -> Result<Option<H256>>;
}

/// Current status of the blockchain's head.
#[derive(Debug, Eq, PartialEq)]
pub struct ChainInfo {
    /// Best block hash.
    pub best_hash: H256,
    /// Best block number.
    pub best_number: reth_primitives::BlockNumber,
    /// Last block that was finalized.
    pub last_finalized: Option<reth_primitives::BlockNumber>,
    /// Safe block
    pub safe_finalized: Option<reth_primitives::BlockNumber>,
}

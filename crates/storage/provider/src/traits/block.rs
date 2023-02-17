use crate::HeaderProvider;

use super::BlockHashProvider;
use reth_interfaces::Result;
use reth_primitives::{
    rpc::{BlockId, BlockNumber},
    Block, ChainInfo, H256, U256,
};

/// Api trait for fetching `Block` related data.
pub trait BlockProvider: BlockHashProvider + HeaderProvider + Send + Sync {
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
            BlockId::Hash(hash) => Ok(Some(H256(hash.0))),
            BlockId::Number(num) => {
                if matches!(num, BlockNumber::Latest) {
                    return Ok(Some(self.chain_info()?.best_hash))
                }
                self.convert_block_number(num)?
                    .map(|num| self.block_hash(U256::from(num)))
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
            BlockId::Hash(hash) => self.block_number(H256(hash.0)),
            BlockId::Number(num) => self.convert_block_number(num),
        }
    }

    /// Gets the `Block` for the given hash. Returns `None` if no block with this hash exists.
    fn block_number(&self, hash: H256) -> Result<Option<reth_primitives::BlockNumber>>;
}

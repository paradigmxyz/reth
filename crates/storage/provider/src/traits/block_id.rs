use super::BlockHashProvider;
use reth_interfaces::Result;
use reth_primitives::{BlockId, BlockNumberOrTag, ChainInfo, H256, U256};

/// Client trait for transforming [BlockId].
#[auto_impl::auto_impl(&, Arc)]
pub trait BlockIdProvider: BlockHashProvider + Send + Sync {
    /// Returns the current info for the chain.
    fn chain_info(&self) -> Result<ChainInfo>;

    /// Converts the `BlockNumberOrTag` variants.
    fn convert_block_number(
        &self,
        num: BlockNumberOrTag,
    ) -> Result<Option<reth_primitives::BlockNumber>> {
        let num = match num {
            BlockNumberOrTag::Latest => self.chain_info()?.best_number,
            BlockNumberOrTag::Earliest => 0,
            BlockNumberOrTag::Pending => return Ok(None),
            BlockNumberOrTag::Number(num) => num,
            BlockNumberOrTag::Finalized => return Ok(self.chain_info()?.last_finalized),
            BlockNumberOrTag::Safe => return Ok(self.chain_info()?.safe_finalized),
        };
        Ok(Some(num))
    }

    /// Get the hash of the block by matching the given id.
    fn block_hash_for_id(&self, block_id: BlockId) -> Result<Option<H256>> {
        match block_id {
            BlockId::Hash(hash) => Ok(Some(hash.into())),
            BlockId::Number(num) => {
                if matches!(num, BlockNumberOrTag::Latest) {
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
            BlockId::Hash(hash) => self.block_number(hash.into()),
            BlockId::Number(num) => self.convert_block_number(num),
        }
    }

    /// Gets the `Block` for the given hash. Returns `None` if no block with this hash exists.
    fn block_number(&self, hash: H256) -> Result<Option<reth_primitives::BlockNumber>>;
}

use super::BlockHashReader;
use reth_interfaces::{provider::ProviderError, Result};
use reth_primitives::{BlockHashOrNumber, BlockId, BlockNumber, BlockNumberOrTag, ChainInfo, H256};

/// Client trait for getting important block numbers (such as the latest block number), converting
/// block hashes to numbers, and fetching a block hash from its block number.
///
/// This trait also supports fetching block hashes and block numbers from a [BlockHashOrNumber].
#[auto_impl::auto_impl(&, Arc)]
pub trait BlockNumReader: BlockHashReader + Send + Sync {
    /// Returns the current info for the chain.
    fn chain_info(&self) -> Result<ChainInfo>;

    /// Returns the best block number in the chain.
    fn best_block_number(&self) -> Result<BlockNumber>;

    /// Returns the last block number associated with the last canonical header in the database.
    fn last_block_number(&self) -> Result<BlockNumber>;

    /// Gets the `BlockNumber` for the given hash. Returns `None` if no block with this hash exists.
    fn block_number(&self, hash: H256) -> Result<Option<BlockNumber>>;

    /// Gets the block number for the given `BlockHashOrNumber`. Returns `None` if no block with
    /// this hash exists. If the `BlockHashOrNumber` is a `Number`, it is returned as is.
    fn convert_hash_or_number(&self, id: BlockHashOrNumber) -> Result<Option<BlockNumber>> {
        match id {
            BlockHashOrNumber::Hash(hash) => self.block_number(hash),
            BlockHashOrNumber::Number(num) => Ok(Some(num)),
        }
    }

    /// Gets the block hash for the given `BlockHashOrNumber`. Returns `None` if no block with this
    /// number exists. If the `BlockHashOrNumber` is a `Hash`, it is returned as is.
    fn convert_number(&self, id: BlockHashOrNumber) -> Result<Option<H256>> {
        match id {
            BlockHashOrNumber::Hash(hash) => Ok(Some(hash)),
            BlockHashOrNumber::Number(num) => self.block_hash(num),
        }
    }
}

/// Client trait for transforming [BlockId] into block numbers or hashes.
///
/// Types that implement this trait must be able to resolve all variants of [BlockNumberOrTag] to
/// block numbers or hashes. Automatic implementations for resolving [BlockNumberOrTag] variants
/// are provided if the type implements the `pending_block_num_hash`, `finalized_block_num`, and
/// `safe_block_num` methods.
///
/// The resulting block numbers can be converted to hashes using the underlying [BlockNumReader]
/// methods, and vice versa.
#[auto_impl::auto_impl(&, Arc)]
pub trait BlockIdReader: BlockNumReader + Send + Sync {
    /// Converts the `BlockNumberOrTag` variants to a block number.
    fn convert_block_number(&self, num: BlockNumberOrTag) -> Result<Option<BlockNumber>> {
        let num = match num {
            BlockNumberOrTag::Latest => self.best_block_number()?,
            BlockNumberOrTag::Earliest => 0,
            BlockNumberOrTag::Pending => {
                return self
                    .pending_block_num_hash()
                    .map(|res_opt| res_opt.map(|num_hash| num_hash.number))
            }
            BlockNumberOrTag::Number(num) => num,
            BlockNumberOrTag::Finalized => match self.finalized_block_number()? {
                Some(block_number) => block_number,
                None => return Err(ProviderError::FinalizedBlockNotFound.into()),
            },
            BlockNumberOrTag::Safe => match self.safe_block_number()? {
                Some(block_number) => block_number,
                None => return Err(ProviderError::SafeBlockNotFound.into()),
            },
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

                if matches!(num, BlockNumberOrTag::Pending) {
                    return self
                        .pending_block_num_hash()
                        .map(|res_opt| res_opt.map(|num_hash| num_hash.hash))
                }

                self.convert_block_number(num)?
                    .map(|num| self.block_hash(num))
                    .transpose()
                    .map(|maybe_hash| maybe_hash.flatten())
            }
        }
    }

    /// Get the number of the block by matching the given id.
    fn block_number_for_id(&self, block_id: BlockId) -> Result<Option<BlockNumber>> {
        match block_id {
            BlockId::Hash(hash) => self.block_number(hash.into()),
            BlockId::Number(num) => self.convert_block_number(num),
        }
    }

    /// Get the current pending block number and hash.
    fn pending_block_num_hash(&self) -> Result<Option<reth_primitives::BlockNumHash>>;

    /// Get the current safe block number and hash.
    fn safe_block_num_hash(&self) -> Result<Option<reth_primitives::BlockNumHash>>;

    /// Get the current finalized block number and hash.
    fn finalized_block_num_hash(&self) -> Result<Option<reth_primitives::BlockNumHash>>;

    /// Get the safe block number.
    fn safe_block_number(&self) -> Result<Option<BlockNumber>> {
        self.safe_block_num_hash().map(|res_opt| res_opt.map(|num_hash| num_hash.number))
    }

    /// Get the finalized block number.
    fn finalized_block_number(&self) -> Result<Option<BlockNumber>> {
        self.finalized_block_num_hash().map(|res_opt| res_opt.map(|num_hash| num_hash.number))
    }

    /// Get the safe block hash.
    fn safe_block_hash(&self) -> Result<Option<H256>> {
        self.safe_block_num_hash().map(|res_opt| res_opt.map(|num_hash| num_hash.hash))
    }

    /// Get the finalized block hash.
    fn finalized_block_hash(&self) -> Result<Option<H256>> {
        self.finalized_block_num_hash().map(|res_opt| res_opt.map(|num_hash| num_hash.hash))
    }
}

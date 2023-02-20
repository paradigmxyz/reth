use crate::{BlockIdProvider, HeaderProvider, TransactionsProvider};
use reth_interfaces::Result;
use reth_primitives::{Block, BlockId, BlockNumberOrTag, H256};

/// Api trait for fetching `Block` related data.
#[auto_impl::auto_impl(&, Arc)]
pub trait BlockProvider:
    BlockIdProvider + HeaderProvider + TransactionsProvider + Send + Sync
{
    /// Returns the block. Returns `None` if block is not found.
    fn block(&self, id: BlockId) -> Result<Option<Block>>;

    /// Returns the block. Returns `None` if block is not found.
    fn block_by_hash(&self, hash: H256) -> Result<Option<Block>> {
        self.block(hash.into())
    }

    /// Returns the block. Returns `None` if block is not found.
    fn block_by_number_or_tag(&self, num: BlockNumberOrTag) -> Result<Option<Block>> {
        self.block(num.into())
    }

    /// Returns the block. Returns `None` if block is not found.
    fn block_by_number(&self, num: u64) -> Result<Option<Block>> {
        self.block(num.into())
    }
}

use crate::{BlockIdProvider, HeaderProvider, TransactionsProvider};
use reth_interfaces::Result;
use reth_primitives::{
    rpc::{BlockId, BlockNumber},
    Block, H256,
};

/// Api trait for fetching `Block` related data.
#[auto_impl::auto_impl(&, Arc)]
pub trait BlockProvider:
    BlockIdProvider + HeaderProvider + TransactionsProvider + Send + Sync
{
    /// Returns the block. Returns `None` if block is not found.
    fn block(&self, id: BlockId) -> Result<Option<Block>>;

    /// Returns the block. Returns `None` if block is not found.
    fn block_by_hash(&self, hash: H256) -> Result<Option<Block>> {
        // TODO: replace with ruint
        self.block(BlockId::Hash(reth_primitives::rpc::H256::from(hash.0)))
    }

    /// Returns the block. Returns `None` if block is not found.
    fn block_by_number_or_tag(&self, num: BlockNumber) -> Result<Option<Block>> {
        self.block(BlockId::Number(num))
    }

    /// Returns the block. Returns `None` if block is not found.
    fn block_by_number(&self, num: u64) -> Result<Option<Block>> {
        self.block(BlockId::Number(BlockNumber::Number(num.into())))
    }
}

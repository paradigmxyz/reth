use super::BlockIdProvider;
use crate::{HeaderProvider, TransactionsProvider};
use reth_interfaces::Result;
use reth_primitives::{rpc::BlockId, Block};

/// Api trait for fetching `Block` related data.
pub trait BlockProvider:
    BlockIdProvider + HeaderProvider + TransactionsProvider + Send + Sync
{
    /// Returns the block. Returns `None` if block is not found.
    fn block(&self, id: BlockId) -> Result<Option<Block>>;
}

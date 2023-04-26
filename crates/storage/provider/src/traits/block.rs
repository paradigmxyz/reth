use crate::{BlockIdProvider, HeaderProvider, ReceiptProvider, TransactionsProvider};
use reth_interfaces::Result;
use reth_primitives::{Block, BlockId, BlockNumberOrTag, Header, H256};

/// A helper enum that represents the origin of the requested block.
///
/// This helper type's sole purpose is to give the caller more control over from where blocks can be
/// fetched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum BlockSource {
    /// Check all available sources.
    ///
    /// Note: it's expected that looking up pending blocks is faster than looking up blocks in the
    /// database so this prioritizes Pending > Database.
    #[default]
    Any,
    /// The block was fetched from the pending block source, the blockchain tree that buffers
    /// blocks that are not yet finalized.
    Pending,
    /// The block was fetched from the database.
    Database,
}

impl BlockSource {
    /// Returns `true` if the block source is `Pending` or `Any`.
    pub fn is_pending(&self) -> bool {
        matches!(self, BlockSource::Pending | BlockSource::Any)
    }

    /// Returns `true` if the block source is `Database` or `Any`.
    pub fn is_database(&self) -> bool {
        matches!(self, BlockSource::Database | BlockSource::Any)
    }
}

/// Api trait for fetching `Block` related data.
///
/// If not requested otherwise, implementers of this trait should prioritize fetching blocks from
/// the database.
#[auto_impl::auto_impl(&, Arc)]
pub trait BlockProvider:
    BlockIdProvider + HeaderProvider + TransactionsProvider + ReceiptProvider + Send + Sync
{
    /// Tries to find in the given block source.
    ///
    /// Note: this only operates on the hash because the number might be ambiguous.
    ///
    /// Returns `None` if block is not found.
    fn find_block_by_hash(&self, hash: H256, source: BlockSource) -> Result<Option<Block>>;

    /// Returns the block with given id from the database.
    ///
    /// Returns `None` if block is not found.
    fn block(&self, id: BlockId) -> Result<Option<Block>>;

    /// Returns the ommers/uncle headers of the given block from the database.
    ///
    /// Returns `None` if block is not found.
    fn ommers(&self, id: BlockId) -> Result<Option<Vec<Header>>>;

    /// Returns the block with matching hash from the database.
    ///
    /// Returns `None` if block is not found.
    fn block_by_hash(&self, hash: H256) -> Result<Option<Block>> {
        self.block(hash.into())
    }

    /// Returns the block with matching tag from the database
    ///
    /// Returns `None` if block is not found.
    fn block_by_number_or_tag(&self, num: BlockNumberOrTag) -> Result<Option<Block>> {
        self.block(num.into())
    }

    /// Returns the block with matching number from database.
    ///
    /// Returns `None` if block is not found.
    fn block_by_number(&self, num: u64) -> Result<Option<Block>> {
        self.block(num.into())
    }
}

use crate::{
    BlockIdProvider, BlockNumProvider, HeaderProvider, ReceiptProvider, TransactionsProvider,
};
use reth_db::models::StoredBlockBodyIndices;
use reth_interfaces::Result;
use reth_primitives::{
    Block, BlockHashOrNumber, BlockId, BlockNumberOrTag, Header, SealedBlock, SealedHeader, H256,
};

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
    BlockNumProvider + HeaderProvider + TransactionsProvider + ReceiptProvider + Send + Sync
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
    fn block(&self, id: BlockHashOrNumber) -> Result<Option<Block>>;

    /// Returns the pending block if available
    ///
    /// Note: This returns a [SealedBlock] because it's expected that this is sealed by the provider
    /// and the caller does not know the hash.
    fn pending_block(&self) -> Result<Option<SealedBlock>>;

    /// Returns the ommers/uncle headers of the given block from the database.
    ///
    /// Returns `None` if block is not found.
    fn ommers(&self, id: BlockHashOrNumber) -> Result<Option<Vec<Header>>>;

    /// Returns the block with matching hash from the database.
    ///
    /// Returns `None` if block is not found.
    fn block_by_hash(&self, hash: H256) -> Result<Option<Block>> {
        self.block(hash.into())
    }

    /// Returns the block with matching number from database.
    ///
    /// Returns `None` if block is not found.
    fn block_by_number(&self, num: u64) -> Result<Option<Block>> {
        self.block(num.into())
    }

    /// Returns the block body indices with matching number from database.
    ///
    /// Returns `None` if block is not found.
    fn block_body_indices(&self, num: u64) -> Result<Option<StoredBlockBodyIndices>>;
}

/// Trait extension for `BlockProvider`, for types that implement `BlockId` conversion.
///
/// The `BlockProvider` trait should be implemented on types that can retrieve a block from either
/// a block number or hash. However, it might be desirable to fetch a block from a `BlockId` type,
/// which can be a number, hash, or tag such as `BlockNumberOrTag::Safe`.
///
/// Resolving tags requires keeping track of block hashes or block numbers associated with the tag,
/// so this trait can only be implemented for types that implement `BlockIdProvider`. The
/// `BlockIdProvider` methods should be used to resolve `BlockId`s to block numbers or hashes, and
/// retrieving the block should be done using the type's `BlockProvider` methods.
#[auto_impl::auto_impl(&, Arc)]
pub trait BlockProviderIdExt: BlockProvider + BlockIdProvider {
    /// Returns the block with matching tag from the database
    ///
    /// Returns `None` if block is not found.
    fn block_by_number_or_tag(&self, id: BlockNumberOrTag) -> Result<Option<Block>> {
        self.convert_block_number(id)?.map_or_else(|| Ok(None), |num| self.block(num.into()))
    }

    /// Returns the pending block header if available
    ///
    /// Note: This returns a [SealedHeader] because it's expected that this is sealed by the
    /// provider and the caller does not know the hash.
    fn pending_header(&self) -> Result<Option<SealedHeader>> {
        self.sealed_header_by_id(BlockNumberOrTag::Pending.into())
    }

    /// Returns the latest block header if available
    ///
    /// Note: This returns a [SealedHeader] because it's expected that this is sealed by the
    /// provider and the caller does not know the hash.
    fn latest_header(&self) -> Result<Option<SealedHeader>> {
        self.sealed_header_by_id(BlockNumberOrTag::Latest.into())
    }

    /// Returns the safe block header if available
    ///
    /// Note: This returns a [SealedHeader] because it's expected that this is sealed by the
    /// provider and the caller does not know the hash.
    fn safe_header(&self) -> Result<Option<SealedHeader>> {
        self.sealed_header_by_id(BlockNumberOrTag::Safe.into())
    }

    /// Returns the finalized block header if available
    ///
    /// Note: This returns a [SealedHeader] because it's expected that this is sealed by the
    /// provider and the caller does not know the hash.
    fn finalized_header(&self) -> Result<Option<SealedHeader>> {
        self.sealed_header_by_id(BlockNumberOrTag::Finalized.into())
    }

    /// Returns the block with the matching `BlockId` from the database.
    ///
    /// Returns `None` if block is not found.
    fn block_by_id(&self, id: BlockId) -> Result<Option<Block>>;

    /// Returns the header with matching tag from the database
    ///
    /// Returns `None` if header is not found.
    fn header_by_number_or_tag(&self, id: BlockNumberOrTag) -> Result<Option<Header>> {
        self.convert_block_number(id)?
            .map_or_else(|| Ok(None), |num| self.header_by_hash_or_number(num.into()))
    }

    /// Returns the header with matching tag from the database
    ///
    /// Returns `None` if header is not found.
    fn sealed_header_by_number_or_tag(&self, id: BlockNumberOrTag) -> Result<Option<SealedHeader>> {
        self.convert_block_number(id)?
            .map_or_else(|| Ok(None), |num| self.header_by_hash_or_number(num.into()))?
            .map_or_else(|| Ok(None), |h| Ok(Some(h.seal_slow())))
    }

    /// Returns the sealed header with the matching `BlockId` from the database.
    ///
    /// Returns `None` if header is not found.
    fn sealed_header_by_id(&self, id: BlockId) -> Result<Option<SealedHeader>>;

    /// Returns the header with the matching `BlockId` from the database.
    ///
    /// Returns `None` if header is not found.
    fn header_by_id(&self, id: BlockId) -> Result<Option<Header>>;

    /// Returns the ommers with the matching tag from the database.
    fn ommers_by_number_or_tag(&self, id: BlockNumberOrTag) -> Result<Option<Vec<Header>>> {
        self.convert_block_number(id)?.map_or_else(|| Ok(None), |num| self.ommers(num.into()))
    }

    /// Returns the ommers with the matching `BlockId` from the database.
    ///
    /// Returns `None` if block is not found.
    fn ommers_by_id(&self, id: BlockId) -> Result<Option<Vec<Header>>>;
}

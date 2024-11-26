use crate::{
    BlockNumReader, HeaderProvider, ReceiptProvider, ReceiptProviderIdExt, TransactionVariant,
    TransactionsProvider, WithdrawalsProvider,
};
use alloy_consensus::Header;
use alloy_eips::{BlockHashOrNumber, BlockId, BlockNumberOrTag};
use alloy_primitives::{BlockNumber, B256};
use reth_db_models::StoredBlockBodyIndices;
use reth_primitives::{
    BlockWithSenders, Receipt, SealedBlockFor, SealedBlockWithSenders, SealedHeader,
};
use reth_storage_errors::provider::ProviderResult;
use std::ops::RangeInclusive;

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
    /// blocks that are not yet part of the canonical chain.
    Pending,
    /// The block must be part of the canonical chain.
    Canonical,
}

impl BlockSource {
    /// Returns `true` if the block source is `Pending` or `Any`.
    pub const fn is_pending(&self) -> bool {
        matches!(self, Self::Pending | Self::Any)
    }

    /// Returns `true` if the block source is `Canonical` or `Any`.
    pub const fn is_canonical(&self) -> bool {
        matches!(self, Self::Canonical | Self::Any)
    }
}

/// Api trait for fetching `Block` related data.
///
/// If not requested otherwise, implementers of this trait should prioritize fetching blocks from
/// the database.
pub trait BlockReader:
    BlockNumReader
    + HeaderProvider
    + TransactionsProvider
    + ReceiptProvider
    + WithdrawalsProvider
    + Send
    + Sync
{
    /// The block type this provider reads.
    type Block: reth_primitives_traits::Block<
        Body: reth_primitives_traits::BlockBody<Transaction = Self::Transaction>,
    >;

    /// Tries to find in the given block source.
    ///
    /// Note: this only operates on the hash because the number might be ambiguous.
    ///
    /// Returns `None` if block is not found.
    fn find_block_by_hash(
        &self,
        hash: B256,
        source: BlockSource,
    ) -> ProviderResult<Option<Self::Block>>;

    /// Returns the block with given id from the database.
    ///
    /// Returns `None` if block is not found.
    fn block(&self, id: BlockHashOrNumber) -> ProviderResult<Option<Self::Block>>;

    /// Returns the pending block if available
    ///
    /// Note: This returns a [`SealedBlockFor`] because it's expected that this is sealed by the
    /// provider and the caller does not know the hash.
    fn pending_block(&self) -> ProviderResult<Option<SealedBlockFor<Self::Block>>>;

    /// Returns the pending block if available
    ///
    /// Note: This returns a [`SealedBlockWithSenders`] because it's expected that this is sealed by
    /// the provider and the caller does not know the hash.
    fn pending_block_with_senders(
        &self,
    ) -> ProviderResult<Option<SealedBlockWithSenders<Self::Block>>>;

    /// Returns the pending block and receipts if available.
    #[allow(clippy::type_complexity)]
    fn pending_block_and_receipts(
        &self,
    ) -> ProviderResult<Option<(SealedBlockFor<Self::Block>, Vec<Receipt>)>>;

    /// Returns the ommers/uncle headers of the given block from the database.
    ///
    /// Returns `None` if block is not found.
    fn ommers(&self, id: BlockHashOrNumber) -> ProviderResult<Option<Vec<Header>>>;

    /// Returns the block with matching hash from the database.
    ///
    /// Returns `None` if block is not found.
    fn block_by_hash(&self, hash: B256) -> ProviderResult<Option<Self::Block>> {
        self.block(hash.into())
    }

    /// Returns the block with matching number from database.
    ///
    /// Returns `None` if block is not found.
    fn block_by_number(&self, num: u64) -> ProviderResult<Option<Self::Block>> {
        self.block(num.into())
    }

    /// Returns the block body indices with matching number from database.
    ///
    /// Returns `None` if block is not found.
    fn block_body_indices(&self, num: u64) -> ProviderResult<Option<StoredBlockBodyIndices>>;

    /// Returns the block with senders with matching number or hash from database.
    ///
    /// Returns the block's transactions in the requested variant.
    ///
    /// Returns `None` if block is not found.
    fn block_with_senders(
        &self,
        id: BlockHashOrNumber,
        transaction_kind: TransactionVariant,
    ) -> ProviderResult<Option<BlockWithSenders<Self::Block>>>;

    /// Returns the sealed block with senders with matching number or hash from database.
    ///
    /// Returns the block's transactions in the requested variant.
    ///
    /// Returns `None` if block is not found.
    fn sealed_block_with_senders(
        &self,
        id: BlockHashOrNumber,
        transaction_kind: TransactionVariant,
    ) -> ProviderResult<Option<SealedBlockWithSenders<Self::Block>>>;

    /// Returns all blocks in the given inclusive range.
    ///
    /// Note: returns only available blocks
    fn block_range(&self, range: RangeInclusive<BlockNumber>) -> ProviderResult<Vec<Self::Block>>;

    /// Returns a range of blocks from the database, along with the senders of each
    /// transaction in the blocks.
    fn block_with_senders_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<BlockWithSenders<Self::Block>>>;

    /// Returns a range of sealed blocks from the database, along with the senders of each
    /// transaction in the blocks.
    fn sealed_block_with_senders_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<SealedBlockWithSenders<Self::Block>>>;
}

impl<T: BlockReader> BlockReader for std::sync::Arc<T> {
    type Block = T::Block;

    fn find_block_by_hash(
        &self,
        hash: B256,
        source: BlockSource,
    ) -> ProviderResult<Option<Self::Block>> {
        T::find_block_by_hash(self, hash, source)
    }
    fn block(&self, id: BlockHashOrNumber) -> ProviderResult<Option<Self::Block>> {
        T::block(self, id)
    }
    fn pending_block(&self) -> ProviderResult<Option<SealedBlockFor<Self::Block>>> {
        T::pending_block(self)
    }
    fn pending_block_with_senders(
        &self,
    ) -> ProviderResult<Option<SealedBlockWithSenders<Self::Block>>> {
        T::pending_block_with_senders(self)
    }
    fn pending_block_and_receipts(
        &self,
    ) -> ProviderResult<Option<(SealedBlockFor<Self::Block>, Vec<Receipt>)>> {
        T::pending_block_and_receipts(self)
    }
    fn ommers(&self, id: BlockHashOrNumber) -> ProviderResult<Option<Vec<Header>>> {
        T::ommers(self, id)
    }
    fn block_by_hash(&self, hash: B256) -> ProviderResult<Option<Self::Block>> {
        T::block_by_hash(self, hash)
    }
    fn block_by_number(&self, num: u64) -> ProviderResult<Option<Self::Block>> {
        T::block_by_number(self, num)
    }
    fn block_body_indices(&self, num: u64) -> ProviderResult<Option<StoredBlockBodyIndices>> {
        T::block_body_indices(self, num)
    }
    fn block_with_senders(
        &self,
        id: BlockHashOrNumber,
        transaction_kind: TransactionVariant,
    ) -> ProviderResult<Option<BlockWithSenders<Self::Block>>> {
        T::block_with_senders(self, id, transaction_kind)
    }
    fn sealed_block_with_senders(
        &self,
        id: BlockHashOrNumber,
        transaction_kind: TransactionVariant,
    ) -> ProviderResult<Option<SealedBlockWithSenders<Self::Block>>> {
        T::sealed_block_with_senders(self, id, transaction_kind)
    }
    fn block_range(&self, range: RangeInclusive<BlockNumber>) -> ProviderResult<Vec<Self::Block>> {
        T::block_range(self, range)
    }
    fn block_with_senders_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<BlockWithSenders<Self::Block>>> {
        T::block_with_senders_range(self, range)
    }
    fn sealed_block_with_senders_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<SealedBlockWithSenders<Self::Block>>> {
        T::sealed_block_with_senders_range(self, range)
    }
}

impl<T: BlockReader> BlockReader for &T {
    type Block = T::Block;

    fn find_block_by_hash(
        &self,
        hash: B256,
        source: BlockSource,
    ) -> ProviderResult<Option<Self::Block>> {
        T::find_block_by_hash(self, hash, source)
    }
    fn block(&self, id: BlockHashOrNumber) -> ProviderResult<Option<Self::Block>> {
        T::block(self, id)
    }
    fn pending_block(&self) -> ProviderResult<Option<SealedBlockFor<Self::Block>>> {
        T::pending_block(self)
    }
    fn pending_block_with_senders(
        &self,
    ) -> ProviderResult<Option<SealedBlockWithSenders<Self::Block>>> {
        T::pending_block_with_senders(self)
    }
    fn pending_block_and_receipts(
        &self,
    ) -> ProviderResult<Option<(SealedBlockFor<Self::Block>, Vec<Receipt>)>> {
        T::pending_block_and_receipts(self)
    }
    fn ommers(&self, id: BlockHashOrNumber) -> ProviderResult<Option<Vec<Header>>> {
        T::ommers(self, id)
    }
    fn block_by_hash(&self, hash: B256) -> ProviderResult<Option<Self::Block>> {
        T::block_by_hash(self, hash)
    }
    fn block_by_number(&self, num: u64) -> ProviderResult<Option<Self::Block>> {
        T::block_by_number(self, num)
    }
    fn block_body_indices(&self, num: u64) -> ProviderResult<Option<StoredBlockBodyIndices>> {
        T::block_body_indices(self, num)
    }
    fn block_with_senders(
        &self,
        id: BlockHashOrNumber,
        transaction_kind: TransactionVariant,
    ) -> ProviderResult<Option<BlockWithSenders<Self::Block>>> {
        T::block_with_senders(self, id, transaction_kind)
    }
    fn sealed_block_with_senders(
        &self,
        id: BlockHashOrNumber,
        transaction_kind: TransactionVariant,
    ) -> ProviderResult<Option<SealedBlockWithSenders<Self::Block>>> {
        T::sealed_block_with_senders(self, id, transaction_kind)
    }
    fn block_range(&self, range: RangeInclusive<BlockNumber>) -> ProviderResult<Vec<Self::Block>> {
        T::block_range(self, range)
    }
    fn block_with_senders_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<BlockWithSenders<Self::Block>>> {
        T::block_with_senders_range(self, range)
    }
    fn sealed_block_with_senders_range(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<SealedBlockWithSenders<Self::Block>>> {
        T::sealed_block_with_senders_range(self, range)
    }
}

/// Trait extension for `BlockReader`, for types that implement `BlockId` conversion.
///
/// The `BlockReader` trait should be implemented on types that can retrieve a block from either
/// a block number or hash. However, it might be desirable to fetch a block from a `BlockId` type,
/// which can be a number, hash, or tag such as `BlockNumberOrTag::Safe`.
///
/// Resolving tags requires keeping track of block hashes or block numbers associated with the tag,
/// so this trait can only be implemented for types that implement `BlockIdReader`. The
/// `BlockIdReader` methods should be used to resolve `BlockId`s to block numbers or hashes, and
/// retrieving the block should be done using the type's `BlockReader` methods.
pub trait BlockReaderIdExt: BlockReader + ReceiptProviderIdExt {
    /// Returns the block with matching tag from the database
    ///
    /// Returns `None` if block is not found.
    fn block_by_number_or_tag(&self, id: BlockNumberOrTag) -> ProviderResult<Option<Self::Block>> {
        self.convert_block_number(id)?.map_or_else(|| Ok(None), |num| self.block(num.into()))
    }

    /// Returns the pending block header if available
    ///
    /// Note: This returns a [`SealedHeader`] because it's expected that this is sealed by the
    /// provider and the caller does not know the hash.
    fn pending_header(&self) -> ProviderResult<Option<SealedHeader>> {
        self.sealed_header_by_id(BlockNumberOrTag::Pending.into())
    }

    /// Returns the latest block header if available
    ///
    /// Note: This returns a [`SealedHeader`] because it's expected that this is sealed by the
    /// provider and the caller does not know the hash.
    fn latest_header(&self) -> ProviderResult<Option<SealedHeader>> {
        self.sealed_header_by_id(BlockNumberOrTag::Latest.into())
    }

    /// Returns the safe block header if available
    ///
    /// Note: This returns a [`SealedHeader`] because it's expected that this is sealed by the
    /// provider and the caller does not know the hash.
    fn safe_header(&self) -> ProviderResult<Option<SealedHeader>> {
        self.sealed_header_by_id(BlockNumberOrTag::Safe.into())
    }

    /// Returns the finalized block header if available
    ///
    /// Note: This returns a [`SealedHeader`] because it's expected that this is sealed by the
    /// provider and the caller does not know the hash.
    fn finalized_header(&self) -> ProviderResult<Option<SealedHeader>> {
        self.sealed_header_by_id(BlockNumberOrTag::Finalized.into())
    }

    /// Returns the block with the matching [`BlockId`] from the database.
    ///
    /// Returns `None` if block is not found.
    fn block_by_id(&self, id: BlockId) -> ProviderResult<Option<Self::Block>>;

    /// Returns the block with senders with matching [`BlockId`].
    ///
    /// Returns the block's transactions in the requested variant.
    ///
    /// Returns `None` if block is not found.
    fn block_with_senders_by_id(
        &self,
        id: BlockId,
        transaction_kind: TransactionVariant,
    ) -> ProviderResult<Option<BlockWithSenders<Self::Block>>> {
        match id {
            BlockId::Hash(hash) => {
                self.block_with_senders(hash.block_hash.into(), transaction_kind)
            }
            BlockId::Number(num) => self.convert_block_number(num)?.map_or_else(
                || Ok(None),
                |num| self.block_with_senders(num.into(), transaction_kind),
            ),
        }
    }

    /// Returns the header with matching tag from the database
    ///
    /// Returns `None` if header is not found.
    fn header_by_number_or_tag(&self, id: BlockNumberOrTag) -> ProviderResult<Option<Header>> {
        self.convert_block_number(id)?
            .map_or_else(|| Ok(None), |num| self.header_by_hash_or_number(num.into()))
    }

    /// Returns the header with matching tag from the database
    ///
    /// Returns `None` if header is not found.
    fn sealed_header_by_number_or_tag(
        &self,
        id: BlockNumberOrTag,
    ) -> ProviderResult<Option<SealedHeader>> {
        self.convert_block_number(id)?
            .map_or_else(|| Ok(None), |num| self.header_by_hash_or_number(num.into()))?
            .map_or_else(|| Ok(None), |h| Ok(Some(SealedHeader::seal(h))))
    }

    /// Returns the sealed header with the matching `BlockId` from the database.
    ///
    /// Returns `None` if header is not found.
    fn sealed_header_by_id(&self, id: BlockId) -> ProviderResult<Option<SealedHeader>>;

    /// Returns the header with the matching `BlockId` from the database.
    ///
    /// Returns `None` if header is not found.
    fn header_by_id(&self, id: BlockId) -> ProviderResult<Option<Header>>;

    /// Returns the ommers with the matching tag from the database.
    fn ommers_by_number_or_tag(&self, id: BlockNumberOrTag) -> ProviderResult<Option<Vec<Header>>> {
        self.convert_block_number(id)?.map_or_else(|| Ok(None), |num| self.ommers(num.into()))
    }

    /// Returns the ommers with the matching `BlockId` from the database.
    ///
    /// Returns `None` if block is not found.
    fn ommers_by_id(&self, id: BlockId) -> ProviderResult<Option<Vec<Header>>>;
}

/// Functionality to read the last known chain blocks from the database.
pub trait ChainStateBlockReader: Send + Sync {
    /// Returns the last finalized block number.
    ///
    /// If no finalized block has been written yet, this returns `None`.
    fn last_finalized_block_number(&self) -> ProviderResult<Option<BlockNumber>>;
    /// Returns the last safe block number.
    ///
    /// If no safe block has been written yet, this returns `None`.
    fn last_safe_block_number(&self) -> ProviderResult<Option<BlockNumber>>;
}

/// Functionality to write the last known chain blocks to the database.
pub trait ChainStateBlockWriter: Send + Sync {
    /// Saves the given finalized block number in the DB.
    fn save_finalized_block_number(&self, block_number: BlockNumber) -> ProviderResult<()>;

    /// Saves the given safe block number in the DB.
    fn save_safe_block_number(&self, block_number: BlockNumber) -> ProviderResult<()>;
}

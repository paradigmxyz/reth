use crate::BlockIdReader;
use alloc::vec::Vec;
use alloy_eips::{BlockHashOrNumber, BlockId, BlockNumberOrTag};
use alloy_primitives::{BlockNumber, TxHash, TxNumber};
use core::ops::{RangeBounds, RangeInclusive};
use reth_primitives_traits::Receipt;
use reth_storage_errors::provider::ProviderResult;

/// A helper type alias to access [`ReceiptProvider::Receipt`].
pub type ProviderReceipt<P> = <P as ReceiptProvider>::Receipt;

/// Client trait for fetching receipt data.
#[auto_impl::auto_impl(&, Arc)]
pub trait ReceiptProvider: Send + Sync {
    /// The receipt type.
    type Receipt: Receipt;

    /// Get receipt by transaction number
    ///
    /// Returns `None` if the transaction is not found.
    fn receipt(&self, id: TxNumber) -> ProviderResult<Option<Self::Receipt>>;

    /// Get receipt by transaction hash.
    ///
    /// Returns `None` if the transaction is not found.
    fn receipt_by_hash(&self, hash: TxHash) -> ProviderResult<Option<Self::Receipt>>;

    /// Get receipts by block num or hash.
    ///
    /// Returns `None` if the block is not found.
    fn receipts_by_block(
        &self,
        block: BlockHashOrNumber,
    ) -> ProviderResult<Option<Vec<Self::Receipt>>>;

    /// Get receipts by tx range.
    fn receipts_by_tx_range(
        &self,
        range: impl RangeBounds<TxNumber>,
    ) -> ProviderResult<Vec<Self::Receipt>>;

    /// Get receipts by block range.
    ///
    /// Returns a vector where each element contains all receipts for a block in the range.
    /// The outer vector index corresponds to blocks in the range (`block_range.start()` + index).
    /// Empty blocks will have empty inner vectors.
    ///
    /// This is more efficient than calling `receipts_by_block` multiple times for contiguous ranges
    /// because it can leverage the underlying `receipts_by_tx_range` for the entire transaction
    /// span.
    fn receipts_by_block_range(
        &self,
        block_range: RangeInclusive<BlockNumber>,
    ) -> ProviderResult<Vec<Vec<Self::Receipt>>>;
}

/// Trait extension for `ReceiptProvider`, for types that implement `BlockId` conversion.
///
/// The `Receipt` trait should be implemented on types that can retrieve receipts from either
/// a block number or hash. However, it might be desirable to fetch receipts from a `BlockId` type,
/// which can be a number, hash, or tag such as `BlockNumberOrTag::Safe`.
///
/// Resolving tags requires keeping track of block hashes or block numbers associated with the tag,
/// so this trait can only be implemented for types that implement `BlockIdReader`. The
/// `BlockIdReader` methods should be used to resolve `BlockId`s to block numbers or hashes, and
/// retrieving the receipts should be done using the type's `ReceiptProvider` methods.
pub trait ReceiptProviderIdExt: ReceiptProvider + BlockIdReader {
    /// Get receipt by block id
    fn receipts_by_block_id(&self, block: BlockId) -> ProviderResult<Option<Vec<Self::Receipt>>> {
        let id = match block {
            BlockId::Hash(hash) => BlockHashOrNumber::Hash(hash.block_hash),
            BlockId::Number(num_tag) => {
                if let Some(num) = self.convert_block_number(num_tag)? {
                    BlockHashOrNumber::Number(num)
                } else {
                    return Ok(None)
                }
            }
        };

        self.receipts_by_block(id)
    }

    /// Returns the block with the matching `BlockId` from the database.
    ///
    /// Returns `None` if block is not found.
    fn receipts_by_number_or_tag(
        &self,
        number_or_tag: BlockNumberOrTag,
    ) -> ProviderResult<Option<Vec<Self::Receipt>>> {
        self.receipts_by_block_id(number_or_tag.into())
    }
}

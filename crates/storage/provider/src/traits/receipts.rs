use reth_interfaces::Result;
use reth_primitives::{BlockHashOrNumber, BlockId, Receipt, TxHash, TxNumber};

use crate::BlockIdProvider;

///  Client trait for fetching [Receipt] data .
#[auto_impl::auto_impl(&, Arc)]
pub trait ReceiptProvider: Send + Sync {
    /// Get receipt by transaction number
    fn receipt(&self, id: TxNumber) -> Result<Option<Receipt>>;

    /// Get receipt by transaction hash.
    fn receipt_by_hash(&self, hash: TxHash) -> Result<Option<Receipt>>;

    /// Get receipts by block num or hash.
    fn receipts_by_block(&self, block: BlockHashOrNumber) -> Result<Option<Vec<Receipt>>>;
}

/// Trait extension for `ReceiptProvider`, for types that implement `BlockId` conversion.
///
/// The `Receipt` trait should be implemented on types that can retrieve receipts from either
/// a block number or hash. However, it might be desirable to fetch receipts from a `BlockId` type,
/// which can be a number, hash, or tag such as `BlockNumberOrTag::Safe`.
///
/// Resolving tags requires keeping track of block hashes or block numbers associated with the tag,
/// so this trait can only be implemented for types that implement `BlockIdProvider`. The
/// `BlockIdProvider` methods should be used to resolve `BlockId`s to block numbers or hashes, and
/// retrieving the receipts should be done using the type's `ReceiptProvider` methods.
pub trait ReceiptProviderIdExt: ReceiptProvider + BlockIdProvider {
    /// Get receipt by block id
    fn receipts_by_block_id(&self, block: BlockId) -> Result<Option<Vec<Receipt>>> {
        // TODO: to implement EIP-1898 at the provider level or not
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
}

impl<T> ReceiptProviderIdExt for T where T: ReceiptProvider + BlockIdProvider {}

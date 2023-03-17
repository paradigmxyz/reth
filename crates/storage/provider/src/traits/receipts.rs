use reth_db::models::receipt::StoredReceipt;
use reth_interfaces::Result;
use reth_primitives::{BlockId, TxHash, TxNumber};

///  Client trait for fetching [Receipt] data .
#[auto_impl::auto_impl(&, Arc)]
pub trait ReceiptProvider: Send + Sync {
    /// Get receipt by transaction number
    fn receipt(&self, id: TxNumber) -> Result<Option<StoredReceipt>>;

    /// Get receipt by transaction hash.
    fn receipt_by_hash(&self, hash: TxHash) -> Result<Option<StoredReceipt>>;

    /// Get receipts by block id.
    fn receipts_by_block(&self, block: BlockId) -> Result<Option<Vec<StoredReceipt>>>;
}

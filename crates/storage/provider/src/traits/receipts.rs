use reth_interfaces::Result;
use reth_primitives::{BlockId, Receipt, TxHash, TxNumber};

///  Client trait for fetching [Receipt] data .
#[auto_impl::auto_impl(&, Arc)]
pub trait ReceiptProvider: Send + Sync {
    /// Get receipt by transaction number
    fn receipt(&self, id: TxNumber) -> Result<Option<Receipt>>;

    /// Get receipt by transaction hash.
    fn receipt_by_hash(&self, hash: TxHash) -> Result<Option<Receipt>>;

    /// Get receipts by block id.
    fn receipts_by_block(&self, block: BlockId) -> Result<Option<Vec<Receipt>>>;
}

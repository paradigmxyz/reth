//! Block related types for RPC API.

use std::sync::Arc;

use alloy_primitives::TxHash;
use reth_primitives_traits::{IndexedTx, NodePrimitives, RecoveredBlock};

/// A type alias for an [`Arc`] wrapped [`RecoveredBlock`].
pub type RecoveredBlockArc<N> = Arc<RecoveredBlock<<N as NodePrimitives>::Block>>;

/// A type alias for an [`Arc`] wrapped vector of [`NodePrimitives::Receipt`].
pub type BlockReceiptsArc<N> = Arc<Vec<<N as NodePrimitives>::Receipt>>;

/// A pair of an [`Arc`] wrapped [`RecoveredBlock`] and its corresponding receipts.
///
/// This type is used throughout the RPC layer to efficiently pass around
/// blocks with their execution receipts, avoiding unnecessary cloning.
#[derive(Debug, Clone)]
pub struct BlockAndReceipts<N: NodePrimitives> {
    /// The recovered block.
    pub block: RecoveredBlockArc<N>,
    /// The receipts for the block.
    pub receipts: BlockReceiptsArc<N>,
}

impl<N: NodePrimitives> BlockAndReceipts<N> {
    /// Creates a new [`BlockAndReceipts`] instance.
    pub const fn new(block: RecoveredBlockArc<N>, receipts: BlockReceiptsArc<N>) -> Self {
        Self { block, receipts }
    }

    /// Finds a transaction by hash and returns it along with its corresponding receipt.
    ///
    /// Returns `None` if the transaction is not found in this block.
    pub fn find_transaction_and_receipt_by_hash(
        &self,
        tx_hash: TxHash,
    ) -> Option<(IndexedTx<'_, N::Block>, &N::Receipt)> {
        let indexed_tx = self.block.find_indexed(tx_hash)?;
        let receipt = self.receipts.get(indexed_tx.index())?;
        Some((indexed_tx, receipt))
    }
}

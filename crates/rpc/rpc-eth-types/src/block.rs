//! Block related types for RPC API.

use std::sync::Arc;

use alloy_consensus::TxReceipt;
use alloy_primitives::TxHash;
use reth_primitives_traits::{
    BlockTy, IndexedTx, NodePrimitives, ReceiptTy, RecoveredBlock, SealedBlock,
};
use reth_rpc_convert::{transaction::ConvertReceiptInput, RpcConvert, RpcTypes};

use crate::utils::calculate_gas_used_and_next_log_index;

/// A pair of an [`Arc`] wrapped [`RecoveredBlock`] and its corresponding receipts.
///
/// This type is used throughout the RPC layer to efficiently pass around
/// blocks with their execution receipts, avoiding unnecessary cloning.
#[derive(Debug, Clone)]
pub struct BlockAndReceipts<N: NodePrimitives> {
    /// The recovered block.
    pub block: Arc<RecoveredBlock<BlockTy<N>>>,
    /// The receipts for the block.
    pub receipts: Arc<Vec<ReceiptTy<N>>>,
}

impl<N: NodePrimitives> BlockAndReceipts<N> {
    /// Creates a new [`BlockAndReceipts`] instance.
    pub const fn new(
        block: Arc<RecoveredBlock<BlockTy<N>>>,
        receipts: Arc<Vec<ReceiptTy<N>>>,
    ) -> Self {
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

    /// Returns the underlying sealed block.
    pub fn sealed_block(&self) -> &SealedBlock<BlockTy<N>> {
        self.block.sealed_block()
    }

    /// Returns the rpc transaction receipt for the given transaction hash if it exists.
    ///
    /// This uses the given converter to turn [`Self::find_transaction_and_receipt_by_hash`] into
    /// the rpc format.
    pub fn find_and_convert_transaction_receipt<C>(
        &self,
        tx_hash: TxHash,
        converter: &C,
    ) -> Option<Result<<C::Network as RpcTypes>::Receipt, C::Error>>
    where
        C: RpcConvert<Primitives = N>,
    {
        let (tx, receipt) = self.find_transaction_and_receipt_by_hash(tx_hash)?;
        match convert_transaction_receipt(
            self.block.as_ref(),
            self.receipts.as_ref(),
            tx,
            receipt,
            converter,
        ) {
            Ok(Some(receipt)) => Some(Ok(receipt)),
            Ok(None) => None,
            Err(err) => Some(Err(err)),
        }
    }
}

/// Converts a transaction and its receipt into the rpc receipt format using the given converter.
pub fn convert_transaction_receipt<N, C>(
    block: &RecoveredBlock<BlockTy<N>>,
    all_receipts: &[ReceiptTy<N>],
    tx: IndexedTx<'_, BlockTy<N>>,
    receipt: &ReceiptTy<N>,
    converter: &C,
) -> Result<Option<<C::Network as RpcTypes>::Receipt>, C::Error>
where
    N: NodePrimitives,
    C: RpcConvert<Primitives = N>,
{
    let meta = tx.meta();
    let (gas_used, next_log_index) =
        calculate_gas_used_and_next_log_index(meta.index, all_receipts);

    let mut receipts = converter.convert_receipts_with_block(
        vec![ConvertReceiptInput {
            tx: tx.recovered_tx(),
            gas_used: receipt.cumulative_gas_used() - gas_used,
            receipt: receipt.clone(),
            next_log_index,
            meta,
        }],
        block.sealed_block(),
    )?;

    Ok(receipts.pop())
}

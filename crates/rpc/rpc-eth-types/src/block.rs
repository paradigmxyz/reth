//! Block related types for RPC API.

use std::sync::Arc;

use alloy_consensus::{transaction::TxHashRef, TxReceipt};
use alloy_primitives::TxHash;
use reth_primitives_traits::{
    Block, BlockBody, BlockTy, IndexedTx, NodePrimitives, ReceiptTy, Recovered, RecoveredBlock,
    SealedBlock,
};
use reth_rpc_convert::{transaction::ConvertReceiptInput, RpcConvert, RpcTypes};

use crate::utils::calculate_gas_used_and_next_log_index;

/// Cached data for a transaction lookup.
#[derive(Debug, Clone)]
pub struct CachedTransaction<B: Block, R> {
    /// The block containing this transaction.
    pub block: Arc<RecoveredBlock<B>>,
    /// Index of the transaction within the block.
    pub tx_index: usize,
    /// Receipts for the block, if available.
    pub receipts: Option<Arc<Vec<R>>>,
}

impl<B: Block, R> CachedTransaction<B, R> {
    /// Creates a new cached transaction entry.
    pub const fn new(
        block: Arc<RecoveredBlock<B>>,
        tx_index: usize,
        receipts: Option<Arc<Vec<R>>>,
    ) -> Self {
        Self { block, tx_index, receipts }
    }

    /// Returns the `Recovered<&T>` transaction at the cached index.
    pub fn recovered_transaction(&self) -> Option<Recovered<&<B::Body as BlockBody>::Transaction>> {
        self.block.recovered_transaction(self.tx_index)
    }

    /// Converts this cached transaction into an RPC receipt using the given converter.
    ///
    /// Returns `None` if receipts are not available or the transaction index is out of bounds.
    pub fn into_receipt<N, C>(
        self,
        converter: &C,
    ) -> Option<Result<<C::Network as RpcTypes>::Receipt, C::Error>>
    where
        N: NodePrimitives<Block = B, Receipt = R>,
        R: TxReceipt + Clone,
        C: RpcConvert<Primitives = N>,
    {
        let receipts = self.receipts?;
        let receipt = receipts.get(self.tx_index)?;
        let tx_hash = *self.block.body().transactions().get(self.tx_index)?.tx_hash();
        let tx = self.block.find_indexed(tx_hash)?;
        convert_transaction_receipt::<N, C>(
            self.block.as_ref(),
            receipts.as_ref(),
            tx,
            receipt,
            converter,
        )
    }
}

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
        convert_transaction_receipt(
            self.block.as_ref(),
            self.receipts.as_ref(),
            tx,
            receipt,
            converter,
        )
    }
}

/// Converts a transaction and its receipt into the rpc receipt format using the given converter.
pub fn convert_transaction_receipt<N, C>(
    block: &RecoveredBlock<BlockTy<N>>,
    all_receipts: &[ReceiptTy<N>],
    tx: IndexedTx<'_, BlockTy<N>>,
    receipt: &ReceiptTy<N>,
    converter: &C,
) -> Option<Result<<C::Network as RpcTypes>::Receipt, C::Error>>
where
    N: NodePrimitives,
    C: RpcConvert<Primitives = N>,
{
    let meta = tx.meta();
    let (gas_used, next_log_index) =
        calculate_gas_used_and_next_log_index(meta.index, all_receipts);

    converter
        .convert_receipts_with_block(
            vec![ConvertReceiptInput {
                tx: tx.recovered_tx(),
                gas_used: receipt.cumulative_gas_used() - gas_used,
                receipt: receipt.clone(),
                next_log_index,
                meta,
            }],
            block.sealed_block(),
        )
        .map(|mut receipts| receipts.pop())
        .transpose()
}

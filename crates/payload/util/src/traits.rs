use crate::transaction::PayloadTransactionsCtx;
use alloy_primitives::Address;
use reth_primitives::TransactionSignedEcRecovered;

/// Iterator that returns transactions for the block building process in the order they should be
/// included in the block.
///
/// Can include transactions from the pool and other sources (alternative pools,
/// sequencer-originated transactions, etc.).
pub trait PayloadTransactions<EVM> {
    /// Returns the next transaction to include in the block.
    fn next(&mut self, ctx: &PayloadTransactionsCtx<EVM>) -> Option<TransactionSignedEcRecovered>;

    /// Exclude descendants of the transaction with given sender and nonce from the iterator,
    /// because this transaction won't be included in the block.
    fn mark_invalid(&mut self, sender: Address, nonce: u64);
}

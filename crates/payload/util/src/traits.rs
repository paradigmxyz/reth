use alloy_primitives::Address;
use reth_primitives::RecoveredTx;

/// Iterator that returns transactions for the block building process in the order they should be
/// included in the block.
///
/// Can include transactions from the pool and other sources (alternative pools,
/// sequencer-originated transactions, etc.).
pub trait PayloadTransactions {
    /// Returns the next transaction to include in the block.
    fn next(
        &mut self,
        // In the future, `ctx` can include access to state for block building purposes.
        ctx: (),
    ) -> Option<RecoveredTx>;

    /// Exclude descendants of the transaction with given sender and nonce from the iterator,
    /// because this transaction won't be included in the block.
    fn mark_invalid(&mut self, sender: Address, nonce: u64);
}

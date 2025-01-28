use alloy_primitives::Address;
use reth_primitives::Recovered;

/// Iterator that returns transactions for the block building process in the order they should be
/// included in the block.
///
/// Can include transactions from the pool and other sources (alternative pools,
/// sequencer-originated transactions, etc.).
pub trait PayloadTransactions {
    /// The transaction type this iterator yields.
    type Transaction;

    /// Returns the next transaction to include in the block.
    fn next(
        &mut self,
        // In the future, `ctx` can include access to state for block building purposes.
        ctx: (),
    ) -> Option<Recovered<Self::Transaction>>;

    /// Exclude descendants of the transaction with given sender and nonce from the iterator,
    /// because this transaction won't be included in the block.
    fn mark_invalid(&mut self, sender: Address, nonce: u64);
}

/// [`PayloadTransactions`] implementation that produces nothing.
#[derive(Debug, Clone, Copy)]
pub struct NoopPayloadTransactions<T>(core::marker::PhantomData<T>);

impl<T> Default for NoopPayloadTransactions<T> {
    fn default() -> Self {
        Self(Default::default())
    }
}

impl<T> PayloadTransactions for NoopPayloadTransactions<T> {
    type Transaction = T;

    fn next(&mut self, _ctx: ()) -> Option<Recovered<Self::Transaction>> {
        None
    }

    fn mark_invalid(&mut self, _sender: Address, _nonce: u64) {}
}

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

    /// Marks last yielded transaction as invalid. Implementation is expected to stop yielding any
    /// transactions depending on this one.
    fn mark_invalid(&mut self);
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

    fn mark_invalid(&mut self) {}
}

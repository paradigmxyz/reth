use std::sync::Arc;

use alloy_primitives::{map::HashSet, Address};
use reth_transaction_pool::{PoolTransaction, ValidPoolTransaction};

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
    ) -> Option<Self::Transaction>;

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

    fn next(&mut self, _ctx: ()) -> Option<Self::Transaction> {
        None
    }

    fn mark_invalid(&mut self, _sender: Address, _nonce: u64) {}
}

/// Wrapper struct that allows to convert `BestTransactions` (used in tx pool) to
/// `PayloadTransactions` (used in block composition).
#[derive(Debug)]
pub struct BestPayloadTransactions<T, I>
where
    T: PoolTransaction,
    I: Iterator<Item = Arc<ValidPoolTransaction<T>>>,
{
    invalid: HashSet<Address>,
    best: I,
}

impl<T, I> BestPayloadTransactions<T, I>
where
    T: PoolTransaction,
    I: Iterator<Item = Arc<ValidPoolTransaction<T>>>,
{
    /// Create a new `BestPayloadTransactions` with the given iterator.
    pub fn new(best: I) -> Self {
        Self { invalid: Default::default(), best }
    }
}

impl<T, I> PayloadTransactions for BestPayloadTransactions<T, I>
where
    T: PoolTransaction,
    I: Iterator<Item = Arc<ValidPoolTransaction<T>>>,
{
    type Transaction = T;

    fn next(&mut self, _ctx: ()) -> Option<Self::Transaction> {
        loop {
            let tx = self.best.next()?;
            if self.invalid.contains(tx.sender_ref()) {
                continue
            }
            return Some(tx.transaction.clone())
        }
    }

    fn mark_invalid(&mut self, sender: Address, _nonce: u64) {
        self.invalid.insert(sender);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::{
        BestPayloadTransactions, PayloadTransactions, PayloadTransactionsChain,
        PayloadTransactionsFixed,
    };
    use alloy_primitives::{map::HashSet, Address};
    use reth_transaction_pool::{
        pool::{BestTransactionsWithPrioritizedSenders, PendingPool},
        test_utils::{MockOrdering, MockTransaction, MockTransactionFactory},
        PoolTransaction,
    };

    #[test]
    fn test_best_transactions_chained_iterators() {
        let mut priority_pool = PendingPool::new(MockOrdering::default());
        let mut pool = PendingPool::new(MockOrdering::default());
        let mut f = MockTransactionFactory::default();

        // Block composition
        // ===
        // (1) up to 100 gas: custom top-of-block transaction
        // (2) up to 100 gas: transactions from the priority pool
        // (3) up to 200 gas: only transactions from address A
        // (4) up to 200 gas: only transactions from address B
        // (5) until block gas limit: all transactions from the main pool

        // Notes:
        // - If prioritized addresses overlap, a single transaction will be prioritized twice and
        //   therefore use the per-segment gas limit twice.
        // - Priority pool and main pool must synchronize between each other to make sure there are
        //   no conflicts for the same nonce. For example, in this scenario, pools can't reject
        //   transactions with seemingly incorrect nonces, because previous transactions might be in
        //   the other pool.

        let address_top_of_block = Address::random();
        let address_in_priority_pool = Address::random();
        let address_a = Address::random();
        let address_b = Address::random();
        let address_regular = Address::random();

        // Add transactions to the main pool
        {
            let prioritized_tx_a =
                MockTransaction::eip1559().with_gas_price(5).with_sender(address_a);
            // without our custom logic, B would be prioritized over A due to gas price:
            let prioritized_tx_b =
                MockTransaction::eip1559().with_gas_price(10).with_sender(address_b);
            let regular_tx =
                MockTransaction::eip1559().with_gas_price(15).with_sender(address_regular);
            pool.add_transaction(Arc::new(f.validated(prioritized_tx_a)), 0);
            pool.add_transaction(Arc::new(f.validated(prioritized_tx_b)), 0);
            pool.add_transaction(Arc::new(f.validated(regular_tx)), 0);
        }

        // Add transactions to the priority pool
        {
            let prioritized_tx =
                MockTransaction::eip1559().with_gas_price(0).with_sender(address_in_priority_pool);
            let valid_prioritized_tx = f.validated(prioritized_tx);
            priority_pool.add_transaction(Arc::new(valid_prioritized_tx), 0);
        }

        let mut block = PayloadTransactionsChain::new(
            PayloadTransactionsFixed::single(
                MockTransaction::eip1559().with_sender(address_top_of_block),
            ),
            Some(100),
            PayloadTransactionsChain::new(
                BestPayloadTransactions::new(priority_pool.best()),
                Some(100),
                BestPayloadTransactions::new(BestTransactionsWithPrioritizedSenders::new(
                    HashSet::from([address_a]),
                    200,
                    BestTransactionsWithPrioritizedSenders::new(
                        HashSet::from([address_b]),
                        200,
                        pool.best(),
                    ),
                )),
                None,
            ),
            None,
        );

        assert_eq!(block.next(()).unwrap().sender(), address_top_of_block);
        assert_eq!(block.next(()).unwrap().sender(), address_in_priority_pool);
        assert_eq!(block.next(()).unwrap().sender(), address_a);
        assert_eq!(block.next(()).unwrap().sender(), address_b);
        assert_eq!(block.next(()).unwrap().sender(), address_regular);
    }
}

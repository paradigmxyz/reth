use alloy_consensus::{BlockBody, Header};
use alloy_primitives::{map::HashSet, Address};
use reth_optimism_primitives::{transaction::signed::OpTransaction, DepositReceipt};
use reth_optimism_txpool::interop::MaybeInteropTransaction;
use reth_payload_util::PayloadTransactions;
use reth_primitives_traits::{NodePrimitives, SignedTransaction};
use reth_transaction_pool::{PoolTransaction, ValidPoolTransaction};
use std::sync::Arc;

/// Helper trait to encapsulate common bounds on [`NodePrimitives`] for OP payload builder.
pub trait OpPayloadPrimitives:
    NodePrimitives<
    Receipt: DepositReceipt,
    SignedTx = Self::_TX,
    BlockHeader = Header,
    BlockBody = BlockBody<Self::_TX>,
>
{
    /// Helper AT to bound [`NodePrimitives::Block`] type without causing bound cycle.
    type _TX: SignedTransaction + OpTransaction;
}

impl<Tx, T> OpPayloadPrimitives for T
where
    Tx: SignedTransaction + OpTransaction,
    T: NodePrimitives<
        SignedTx = Tx,
        Receipt: DepositReceipt,
        BlockHeader = Header,
        BlockBody = BlockBody<Tx>,
    >,
{
    type _TX = Tx;
}

/// Wrapper struct that allows to convert `BestTransactions` (used in tx pool) to
/// `PayloadTransactions` (used in block composition).
#[derive(Debug)]
pub struct BestPayloadTransactions<T, I>
where
    T: PoolTransaction + MaybeInteropTransaction,
    I: Iterator<Item = Arc<ValidPoolTransaction<T>>>,
{
    invalid: HashSet<Address>,
    best: I,
}

impl<T, I> BestPayloadTransactions<T, I>
where
    T: PoolTransaction + MaybeInteropTransaction,
    I: Iterator<Item = Arc<ValidPoolTransaction<T>>>,
{
    /// Create a new `BestPayloadTransactions` with the given iterator.
    pub fn new(best: I) -> Self {
        Self { invalid: Default::default(), best }
    }
}

impl<T, I> PayloadTransactions for BestPayloadTransactions<T, I>
where
    T: PoolTransaction + MaybeInteropTransaction,
    I: Iterator<Item = Arc<ValidPoolTransaction<T>>>,
{
    type Transaction = T;

    fn next(&mut self, _ctx: ()) -> Option<Self::Transaction> {
        loop {
            let tx = self.best.next()?;
            if self.invalid.contains(&tx.sender()) {
                continue
            }
            if let Some(interop) = tx.transaction.interop() {
                // TODO: put correct timestamp
                if !interop.is_valid(0) {
                    continue
                }
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

    use crate::BestPayloadTransactions;
    use alloy_primitives::{map::HashSet, Address};
    use reth_payload_util::{
        PayloadTransactions, PayloadTransactionsChain, PayloadTransactionsFixed,
    };
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

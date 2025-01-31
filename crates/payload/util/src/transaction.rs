use crate::PayloadTransactions;
use alloy_consensus::Transaction;
use alloy_primitives::Address;
use reth_primitives::Recovered;

/// An implementation of [`crate::traits::PayloadTransactions`] that yields
/// a pre-defined set of transactions.
///
/// This is useful to put a sequencer-specified set of transactions into the block
/// and compose it with the rest of the transactions.
#[derive(Debug)]
pub struct PayloadTransactionsFixed<T> {
    transactions: Vec<T>,
    index: usize,
}

impl<T> PayloadTransactionsFixed<T> {
    /// Constructs a new [`PayloadTransactionsFixed`].
    pub fn new(transactions: Vec<T>) -> Self {
        Self { transactions, index: Default::default() }
    }

    /// Constructs a new [`PayloadTransactionsFixed`] with a single transaction.
    pub fn single(transaction: T) -> Self {
        Self { transactions: vec![transaction], index: Default::default() }
    }
}

impl<T: Clone> PayloadTransactions for PayloadTransactionsFixed<Recovered<T>> {
    type Transaction = T;

    fn next(&mut self, _ctx: ()) -> Option<Recovered<T>> {
        (self.index < self.transactions.len()).then(|| {
            let tx = self.transactions[self.index].clone();
            self.index += 1;
            tx
        })
    }

    fn mark_invalid(&mut self, _sender: Address, _nonce: u64) {}
}

/// Wrapper over [`crate::traits::PayloadTransactions`] that combines transactions from multiple
/// `PayloadTransactions` iterators and keeps track of the gas for both of iterators.
///
/// We can't use [`Iterator::chain`], because:
/// (a) we need to propagate the `mark_invalid` and `no_updates`
/// (b) we need to keep track of the gas
///
/// Notes that [`PayloadTransactionsChain`] fully drains the first iterator
/// before moving to the second one.
///
/// If the `before` iterator has transactions that are not fitting into the block,
/// the after iterator will get propagated a `mark_invalid` call for each of them.
#[derive(Debug)]
pub struct PayloadTransactionsChain<B: PayloadTransactions, A: PayloadTransactions> {
    /// Iterator that will be used first
    before: B,
    /// Allowed gas for the transactions from `before` iterator. If `None`, no gas limit is
    /// enforced.
    before_max_gas: Option<u64>,
    /// Gas used by the transactions from `before` iterator
    before_gas: u64,
    /// Iterator that will be used after `before` iterator
    after: A,
    /// Allowed gas for the transactions from `after` iterator. If `None`, no gas limit is
    /// enforced.
    after_max_gas: Option<u64>,
    /// Gas used by the transactions from `after` iterator
    after_gas: u64,
}

impl<B: PayloadTransactions, A: PayloadTransactions> PayloadTransactionsChain<B, A> {
    /// Constructs a new [`PayloadTransactionsChain`].
    pub fn new(
        before: B,
        before_max_gas: Option<u64>,
        after: A,
        after_max_gas: Option<u64>,
    ) -> Self {
        Self {
            before,
            before_max_gas,
            before_gas: Default::default(),
            after,
            after_max_gas,
            after_gas: Default::default(),
        }
    }
}

impl<A, B> PayloadTransactions for PayloadTransactionsChain<A, B>
where
    A: PayloadTransactions<Transaction: Transaction>,
    B: PayloadTransactions<Transaction = A::Transaction>,
{
    type Transaction = A::Transaction;

    fn next(&mut self, ctx: ()) -> Option<Recovered<Self::Transaction>> {
        while let Some(tx) = self.before.next(ctx) {
            if let Some(before_max_gas) = self.before_max_gas {
                if self.before_gas + tx.tx().gas_limit() <= before_max_gas {
                    self.before_gas += tx.tx().gas_limit();
                    return Some(tx);
                }
                self.before.mark_invalid(tx.signer(), tx.tx().nonce());
                self.after.mark_invalid(tx.signer(), tx.tx().nonce());
            } else {
                return Some(tx);
            }
        }

        while let Some(tx) = self.after.next(ctx) {
            if let Some(after_max_gas) = self.after_max_gas {
                if self.after_gas + tx.tx().gas_limit() <= after_max_gas {
                    self.after_gas += tx.tx().gas_limit();
                    return Some(tx);
                }
                self.after.mark_invalid(tx.signer(), tx.tx().nonce());
            } else {
                return Some(tx);
            }
        }

        None
    }

    fn mark_invalid(&mut self, sender: Address, nonce: u64) {
        self.before.mark_invalid(sender, nonce);
        self.after.mark_invalid(sender, nonce);
    }
}

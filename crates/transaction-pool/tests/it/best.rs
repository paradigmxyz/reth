//! Best transaction and filter testing

use reth_transaction_pool::{noop::NoopTransactionPool, BestTransactions, TransactionPool};

#[test]
fn test_best_transactions() {
    let noop = NoopTransactionPool::default();
    let mut best = noop.best_transactions().filter_transactions(|_| true);
    assert!(best.next().is_none());
}

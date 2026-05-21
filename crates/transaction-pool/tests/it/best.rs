//! Best transaction and filter testing

use alloy_consensus::conditional::BlockConditionalAttributes;
use reth_transaction_pool::{noop::NoopTransactionPool, BestTransactions, TransactionPool};

#[test]
fn test_best_transactions() {
    let noop = NoopTransactionPool::default();
    let mut best = noop
        .best_transactions()
        .filter_transactions(|_| true)
        .with_block_attributes(BlockConditionalAttributes::new(1, 2))
        .without_blobs()
        .without_updates();
    assert!(best.next().is_none());
}

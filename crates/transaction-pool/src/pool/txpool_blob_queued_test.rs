//! Test that `queued_transactions_count()` includes blob-pool transactions.

use crate::{
    pool::txpool::TxPool,
    test_utils::{MockOrdering, MockTransaction, MockTransactionFactory},
};
use alloy_consensus::Transaction;
use alloy_primitives::U256;

#[test]
fn test_queued_count_omits_blob_pool() {
    let on_chain_balance = U256::MAX;
    let on_chain_nonce = 0u64;
    let mut f = MockTransactionFactory::default();
    let mut pool = TxPool::<MockOrdering>::mock();
    let tx = MockTransaction::eip4844().inc_price().inc_limit();

    // Set block info so the blob tx is underpriced w.r.t. the blob fee, which parks it in the
    // blob pool instead of the pending pool.
    let mut block_info = pool.block_info();
    block_info.pending_blob_fee = Some(tx.max_fee_per_blob_gas().unwrap() + 1);
    pool.set_block_info(block_info);

    let validated = f.validated(tx.clone());
    pool.add_transaction(validated, on_chain_balance, on_chain_nonce, None).unwrap();

    // Confirm classification via public size(): the tx is parked in the blob pool.
    let size = pool.size();
    assert_eq!(size.blob, 1);
    assert_eq!(size.pending, 0);

    // queued_transactions_count() must account for the blob pool.
    assert_eq!(pool.queued_transactions_count(), 1);
}

use assert_matches::assert_matches;
use reth_transaction_pool::{
    test_utils::{MockTransactionFactory, TestPoolBuilder},
    TransactionOrigin, TransactionPool,
};

#[tokio::test(flavor = "multi_thread")]
async fn txpool_new_pending_txs() {
    let txpool = TestPoolBuilder::default();
    let mut mock_tx_factory = MockTransactionFactory::default();
    let transaction = mock_tx_factory.create_eip1559();

    let added_result =
        txpool.add_transaction(TransactionOrigin::External, transaction.transaction.clone()).await;
    assert_matches!(added_result, Ok(hash) if hash == transaction.transaction.get_hash());

    let mut best_txns = txpool.best_transactions();
    assert_matches!(best_txns.next(), Some(tx) if tx.transaction.get_hash() == transaction.transaction.get_hash());
    assert_matches!(best_txns.next(), None);
    let transaction = mock_tx_factory.create_eip1559();
    let added_result =
        txpool.add_transaction(TransactionOrigin::External, transaction.transaction.clone()).await;
    assert_matches!(added_result, Ok(hash) if hash == transaction.transaction.get_hash());
    assert_matches!(best_txns.next(), Some(tx) if tx.transaction.get_hash() == transaction.transaction.get_hash());
}

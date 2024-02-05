//! Blob transaction tests

use reth_transaction_pool::{
    error::PoolErrorKind,
    test_utils::{MockTransaction, MockTransactionFactory, TestPoolBuilder},
    TransactionOrigin, TransactionPool,
};

#[tokio::test(flavor = "multi_thread")]
async fn blobs_exclusive() {
    let txpool = TestPoolBuilder::default();
    let mut mock_tx_factory = MockTransactionFactory::default();
    let blob_tx = mock_tx_factory.create_eip4844();

    let hash = txpool
        .add_transaction(TransactionOrigin::External, blob_tx.transaction.clone())
        .await
        .unwrap();
    assert_eq!(hash, blob_tx.transaction.get_hash());

    let mut best_txns = txpool.best_transactions();
    assert_eq!(best_txns.next().unwrap().transaction.get_hash(), blob_tx.transaction.get_hash());
    assert!(best_txns.next().is_none());

    let eip1559_tx = MockTransaction::eip1559()
        .set_sender(blob_tx.transaction.get_sender())
        .inc_price_by(10_000);

    let res =
        txpool.add_transaction(TransactionOrigin::External, eip1559_tx.clone()).await.unwrap_err();

    assert_eq!(res.hash, eip1559_tx.get_hash());
    match res.kind {
        PoolErrorKind::ExistingConflictingTransactionType(addr, tx_type) => {
            assert_eq!(addr, eip1559_tx.get_sender());
            assert_eq!(tx_type, eip1559_tx.tx_type());
        }
        _ => unreachable!(),
    }
}
